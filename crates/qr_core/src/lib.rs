use std::path::Path;

use anyhow::Context;
use image::{
    imageops::{self, FilterType},
    GrayImage, Luma,
};

const MAX_UPSCALED_PIXELS: u64 = 24_000_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QrDecodeOptions {
    pub force_preprocessing: QrPreprocessingOptions,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QrPreprocessingOptions {
    pub contrast: bool,
    pub brighten: bool,
    pub threshold: bool,
    pub contrast_threshold: bool,
    pub invert: bool,
}

impl QrPreprocessingOptions {
    fn all() -> Self {
        Self {
            contrast: true,
            brighten: true,
            threshold: true,
            contrast_threshold: true,
            invert: true,
        }
    }

    fn any(self) -> bool {
        self.contrast || self.brighten || self.threshold || self.contrast_threshold || self.invert
    }

    fn without(self, other: Self) -> Self {
        Self {
            contrast: self.contrast && !other.contrast,
            brighten: self.brighten && !other.brighten,
            threshold: self.threshold && !other.threshold,
            contrast_threshold: self.contrast_threshold && !other.contrast_threshold,
            invert: self.invert && !other.invert,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedCode {
    pub code_type: CodeType,
    pub decoded_text: String,
    pub decoded_kind: DecodedKind,
}

impl DecodedCode {
    pub fn qr(decoded_text: String) -> Self {
        let decoded_kind = DecodedKind::from_text(&decoded_text);

        Self {
            code_type: CodeType::Qr,
            decoded_text,
            decoded_kind,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeType {
    Qr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodedKind {
    Url,
    Text,
}

impl DecodedKind {
    pub fn from_text(decoded_text: &str) -> Self {
        if decoded_text.starts_with("http://") || decoded_text.starts_with("https://") {
            DecodedKind::Url
        } else {
            DecodedKind::Text
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QrDecodeReport {
    pub decoded_codes: Vec<DecodedCode>,
    pub detected_grids: usize,
    pub failed_grids: usize,
    pub attempts: Vec<QrDecodeAttempt>,
}

impl QrDecodeReport {
    pub fn all_detected_grids_failed(&self) -> bool {
        self.detected_grids > 0 && self.decoded_codes.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrDecodeAttempt {
    pub label: String,
}

impl QrDecodeAttempt {
    fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
        }
    }
}

pub trait QrDecoder: Send + Sync {
    fn decode_path_report(&self, path: &Path) -> anyhow::Result<QrDecodeReport>;

    fn decode_path(&self, path: &Path) -> anyhow::Result<Vec<DecodedCode>> {
        Ok(self.decode_path_report(path)?.decoded_codes)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RqrrDecoder {
    options: QrDecodeOptions,
}

impl RqrrDecoder {
    pub fn new(options: QrDecodeOptions) -> Self {
        Self { options }
    }

    fn decode_path_report_inner(&self, path: &Path) -> anyhow::Result<QrDecodeReport> {
        let image = image::open(path)
            .with_context(|| format!("failed to open image: {}", path.display()))?
            .to_luma8();

        Ok(decode_with_fallbacks(&image, self.options))
    }
}

fn decode_with_fallbacks(image: &GrayImage, options: QrDecodeOptions) -> QrDecodeReport {
    let mut report = QrDecodeReport::default();
    let forced = options.force_preprocessing;

    if merge_attempt_report(
        &mut report,
        decode_image(image.clone(), QrDecodeAttempt::new("元画像")),
    ) && !forced.any()
    {
        return report;
    }

    for (attempt, prepared_image) in preprocessed_images(image, None, forced) {
        merge_attempt_report(&mut report, decode_image(prepared_image, attempt));
    }

    if !report.decoded_codes.is_empty() {
        return report;
    }

    let fallback_preprocessing = QrPreprocessingOptions::all().without(forced);
    for (attempt, prepared_image) in preprocessed_images(image, None, fallback_preprocessing) {
        if merge_attempt_report(&mut report, decode_image(prepared_image, attempt)) {
            return report;
        }
    }

    for (scale_label, upscaled_image) in upscaled_images(image) {
        if merge_attempt_report(
            &mut report,
            decode_image(
                upscaled_image.clone(),
                QrDecodeAttempt::new(format!("{scale_label}拡大")),
            ),
        ) {
            return report;
        }

        for (attempt, prepared_image) in preprocessed_images(
            &upscaled_image,
            Some(scale_label),
            QrPreprocessingOptions::all(),
        ) {
            if merge_attempt_report(&mut report, decode_image(prepared_image, attempt)) {
                return report;
            }
        }
    }

    report
}

fn decode_image(image: GrayImage, attempt: QrDecodeAttempt) -> QrDecodeReport {
    let mut prepared = rqrr::PreparedImage::prepare(image);
    let grids = prepared.detect_grids();
    let detected_grids = grids.len();
    let mut decoded_codes = Vec::new();
    let mut failed_grids = 0;

    for grid in grids {
        match grid.decode() {
            Ok((_metadata, decoded_text)) => decoded_codes.push(DecodedCode::qr(decoded_text)),
            Err(_) => failed_grids += 1,
        }
    }

    QrDecodeReport {
        decoded_codes,
        detected_grids,
        failed_grids,
        attempts: vec![attempt],
    }
}

fn merge_attempt_report(target: &mut QrDecodeReport, report: QrDecodeReport) -> bool {
    let decoded = !report.decoded_codes.is_empty();
    for decoded_code in report.decoded_codes {
        if !target.decoded_codes.iter().any(|existing| {
            existing.code_type == decoded_code.code_type
                && existing.decoded_text == decoded_code.decoded_text
        }) {
            target.decoded_codes.push(decoded_code);
        }
    }
    target.detected_grids += report.detected_grids;
    target.failed_grids += report.failed_grids;
    target.attempts.extend(report.attempts);
    decoded
}

fn preprocessed_images(
    image: &GrayImage,
    scale_label: Option<&str>,
    options: QrPreprocessingOptions,
) -> Vec<(QrDecodeAttempt, GrayImage)> {
    let prefix = scale_label.map(|label| format!("{label}拡大 + "));

    let mut images = Vec::new();

    if options.contrast {
        images.push((
            QrDecodeAttempt::new(format!(
                "{}コントラスト強調",
                prefix.as_deref().unwrap_or("")
            )),
            imageops::contrast(image, 45.0),
        ));
    }
    if options.brighten {
        images.push((
            QrDecodeAttempt::new(format!("{}明るさ補正", prefix.as_deref().unwrap_or(""))),
            imageops::brighten(image, 24),
        ));
    }
    if options.threshold {
        images.push((
            QrDecodeAttempt::new(format!("{}二値化", prefix.as_deref().unwrap_or(""))),
            threshold_image(image, 128),
        ));
    }
    if options.contrast_threshold {
        images.push((
            QrDecodeAttempt::new(format!(
                "{}コントラスト強調 + 二値化",
                prefix.as_deref().unwrap_or("")
            )),
            threshold_image(&imageops::contrast(image, 45.0), 128),
        ));
    }
    if options.invert {
        images.push((
            QrDecodeAttempt::new(format!("{}反転", prefix.as_deref().unwrap_or(""))),
            invert_image(image),
        ));
    }

    images
}

fn upscaled_images(image: &GrayImage) -> Vec<(&'static str, GrayImage)> {
    let mut images = Vec::new();

    if let Some(image) = upscale_image(image, 3, 2) {
        images.push(("1.5x", image));
    }

    if let Some(image) = upscale_image(image, 2, 1) {
        images.push(("2.0x", image));
    }

    images
}

fn upscale_image(image: &GrayImage, numerator: u32, denominator: u32) -> Option<GrayImage> {
    let width = image.width().checked_mul(numerator)? / denominator;
    let height = image.height().checked_mul(numerator)? / denominator;
    let pixels = u64::from(width) * u64::from(height);

    if width == image.width() && height == image.height() {
        return None;
    }

    if pixels > MAX_UPSCALED_PIXELS {
        return None;
    }

    Some(imageops::resize(image, width, height, FilterType::Triangle))
}

fn threshold_image(image: &GrayImage, threshold: u8) -> GrayImage {
    GrayImage::from_fn(image.width(), image.height(), |x, y| {
        if image.get_pixel(x, y)[0] > threshold {
            Luma([255])
        } else {
            Luma([0])
        }
    })
}

fn invert_image(image: &GrayImage) -> GrayImage {
    GrayImage::from_fn(image.width(), image.height(), |x, y| {
        Luma([255 - image.get_pixel(x, y)[0]])
    })
}

impl QrDecoder for RqrrDecoder {
    fn decode_path_report(&self, path: &Path) -> anyhow::Result<QrDecodeReport> {
        self.decode_path_report_inner(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoded_kind_marks_http_and_https_as_url() {
        assert_eq!(
            DecodedKind::from_text("http://example.com"),
            DecodedKind::Url
        );
        assert_eq!(
            DecodedKind::from_text("https://example.com"),
            DecodedKind::Url
        );
    }

    #[test]
    fn decoded_kind_marks_other_values_as_text() {
        assert_eq!(
            DecodedKind::from_text("ftp://example.com"),
            DecodedKind::Text
        );
        assert_eq!(DecodedKind::from_text("hello"), DecodedKind::Text);
    }

    #[test]
    fn blank_image_decodes_to_empty_results() {
        let file = tempfile::Builder::new()
            .prefix("biscotti_blank_")
            .suffix(".png")
            .tempfile()
            .expect("create blank test image file");

        let image = image::GrayImage::from_pixel(64, 64, image::Luma([255]));
        image.save(file.path()).expect("save blank test image");

        let report = QrDecoder::decode_path_report(&RqrrDecoder::default(), file.path())
            .expect("decode blank test image");

        assert!(report.decoded_codes.is_empty());
        assert_eq!(report.detected_grids, 0);
        assert_eq!(report.failed_grids, 0);
        assert!(!report.all_detected_grids_failed());
    }

    #[test]
    fn generated_qr_image_decodes_url() {
        let file = tempfile::Builder::new()
            .prefix("biscotti_qr_")
            .suffix(".png")
            .tempfile()
            .expect("create qr test image file");

        let code = qrcode::QrCode::new(b"https://example.com").expect("create qr code");
        let image = code
            .render::<image::Luma<u8>>()
            .min_dimensions(256, 256)
            .build();
        image.save(file.path()).expect("save qr test image");

        let report = QrDecoder::decode_path_report(&RqrrDecoder::default(), file.path())
            .expect("decode qr test image");

        assert_eq!(report.detected_grids, 1);
        assert_eq!(report.failed_grids, 0);
        assert_eq!(
            report.decoded_codes,
            vec![DecodedCode {
                code_type: CodeType::Qr,
                decoded_text: "https://example.com".to_string(),
                decoded_kind: DecodedKind::Url,
            }]
        );
    }

    #[test]
    fn preprocessing_fallback_decodes_low_contrast_qr() {
        let code =
            qrcode::QrCode::new(b"https://example.com/low-contrast").expect("create qr code");
        let image = code
            .render::<image::Luma<u8>>()
            .dark_color(image::Luma([124]))
            .light_color(image::Luma([132]))
            .min_dimensions(256, 256)
            .build();

        assert!(decode_image(image.clone(), QrDecodeAttempt::new("元画像"))
            .decoded_codes
            .is_empty());

        let report = decode_with_fallbacks(&image, QrDecodeOptions::default());

        assert!(report.decoded_codes.iter().any(|code| {
            code.decoded_text == "https://example.com/low-contrast"
                && code.decoded_kind == DecodedKind::Url
        }));
    }

    #[test]
    fn fallback_pipeline_decodes_small_qr() {
        let code = qrcode::QrCode::new(b"https://example.com/small").expect("create qr code");
        let image = code
            .render::<image::Luma<u8>>()
            .min_dimensions(30, 30)
            .build();

        let report = decode_with_fallbacks(&image, QrDecodeOptions::default());

        assert!(report
            .decoded_codes
            .iter()
            .any(|code| code.decoded_text == "https://example.com/small"));
    }

    #[test]
    fn forced_preprocessing_runs_even_when_original_decodes() {
        let code = qrcode::QrCode::new(b"https://example.com/forced").expect("create qr code");
        let image = code
            .render::<image::Luma<u8>>()
            .min_dimensions(256, 256)
            .build();

        let report = decode_with_fallbacks(
            &image,
            QrDecodeOptions {
                force_preprocessing: QrPreprocessingOptions {
                    contrast: true,
                    ..Default::default()
                },
            },
        );

        assert!(report
            .decoded_codes
            .iter()
            .any(|code| code.decoded_text == "https://example.com/forced"));
        assert_eq!(
            report
                .attempts
                .iter()
                .map(|attempt| attempt.label.as_str())
                .collect::<Vec<_>>(),
            vec!["元画像", "コントラスト強調"]
        );
    }
}
