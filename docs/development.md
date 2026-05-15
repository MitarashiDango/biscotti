# 開発者向けガイド

Biscotti をローカルでビルド・開発する際の手順をまとめたものです。

## 前提環境

| 項目 | 推奨 |
|---|---|
| OS | Windows 10 / 11 (64-bit) |
| Rust ツールチェーン | 安定版 (`stable`)、`rustup` 経由 |
| C/C++ ツール | Visual Studio Build Tools (MSVC、C++ ワークロード) |

`rustup` を未導入の場合は <https://www.rust-lang.org/> からインストールしてください。

## ワークスペース構成

```
biscotti/
├── crates/
│   ├── app_model/           ─ アプリケーション状態と読み取りパイプライン
│   ├── biscotti/            ─ 実行バイナリ + GUI レイヤ (gpui)
│   ├── config_store/        ─ 設定の永続化 (JSON)
│   ├── file_ready_checker/  ─ 書き込み完了判定 (tokio)
│   ├── history_store/       ─ 履歴の永続化 (SQLite, rusqlite)
│   ├── photo_watcher/       ─ ファイルシステム監視 (notify)
│   └── qr_core/             ─ QR デコード (rqrr + 前処理)
├── about.toml / about.hbs   ─ THIRD-PARTY-LICENSES 生成設定
├── LICENSE                  ─ Apache License 2.0
└── THIRD-PARTY-LICENSES     ─ 依存ライブラリのライセンス集約
```

## ビルド

### デフォルト (CLI のみ)

```powershell
cargo build
```

GUI を含まないバイナリが生成されます。引数なしで起動した場合はプレースホルダメッセージのみ表示され、`--notify-probe` などのデバッグサブコマンドが利用できます。

### GUI 込み (通常はこちら)

```powershell
cargo build --features gui
```

リリースビルドの場合は `--release` を追加します。

```powershell
cargo build --release --features gui
```

ビルド成果物は `target/{debug,release}/biscotti.exe` に出力されます。

## 実行

```powershell
cargo run --features gui
```

または、ビルド済みバイナリを直接実行します。

```powershell
.\target\release\biscotti.exe
```

### デバッグ用サブコマンド

`notify` クレートが出力する生イベントを確認したいとき:

```powershell
cargo run -- --notify-probe <監視対象フォルダ>
```

## テスト

```powershell
cargo test --workspace
```

GUI レイヤのテストも含めて実行する場合:

```powershell
cargo test --workspace --features biscotti/gui
```

## Lint / Format

```powershell
cargo fmt --all
cargo clippy --workspace --all-features -- -D warnings
```

## THIRD-PARTY-LICENSES の再生成

依存クレートを追加・更新した際は、`cargo about` で `THIRD-PARTY-LICENSES` を更新します。

### 初回セットアップ

```powershell
cargo install --locked --features cli cargo-about
```

### 再生成コマンド

```powershell
cargo about generate --all-features about.hbs -o THIRD-PARTY-LICENSES
```

`--all-features` を付けることで、`gui` feature 経由で入る `gpui` / `arboard` などの依存も網羅されます。

### ライセンスエラーが出た場合

`error: failed to satisfy license requirements` で停止した際は、未許可のライセンスが新たな依存に含まれています。`about.toml` の `accepted` 配列に追加可能なライセンスかを確認してから追記してください (Apache-2.0 と互換性のないライセンスを安易に追加しないこと)。

## CI / リリース自動化

### CI ([`.github/workflows/ci.yml`](../.github/workflows/ci.yml))

`main` への push および Pull Request 作成時に、Windows runner 上で以下を実行します。

- `cargo fmt --check`
- `cargo clippy --workspace --all-features -- -D warnings`
- `cargo test --workspace --features biscotti/gui`
- `cargo about generate` による `THIRD-PARTY-LICENSES` の再生成 + コミット済みファイルとの差分チェック

`main` ブランチは保護されており、CI が green でない PR はマージできません。

#### THIRD-PARTY-LICENSES がローカルで古い場合

CI で「THIRD-PARTY-LICENSES is out of date」と表示された場合は、以下を実行してコミットを追加してください。

```powershell
cargo about generate --all-features about.hbs -o THIRD-PARTY-LICENSES
git add THIRD-PARTY-LICENSES
git commit -m "chore: regenerate THIRD-PARTY-LICENSES"
```

依存クレートが変わるたびに必要になるため、依存追加・更新の PR では同時に更新する運用にしてください。

### リリース ([`.github/workflows/release.yml`](../.github/workflows/release.yml))

リリースは Git タグの push によって自動化されています。

#### 手順

1. PR で `crates/biscotti/Cargo.toml` のバージョン番号を更新し、main にマージ (CI 必須)。
2. main 最新の commit に `v` プレフィックス付きの Git タグを作成してプッシュします。

   ```powershell
   git checkout main
   git pull
   git tag v0.2.0
   git push origin v0.2.0
   ```

3. タグの push をトリガーに、以下が自動実行されます。
   - タグのバージョンと `Cargo.toml` の整合性チェック (不一致時はエラーで停止)
   - `cargo test --workspace --features biscotti/gui` によるテスト実行
   - `cargo build --release --features gui` によるリリースバイナリのビルド
   - `biscotti.exe` / `LICENSE` / `THIRD-PARTY-LICENSES` (および `NOTICE` がある場合) を ZIP にまとめる
   - SHA-256 チェックサムファイルの生成
   - GitHub Release を **下書き状態**で作成し、ZIP とチェックサムを添付
4. GitHub Releases 画面で内容を確認し、リリースノートを編集してから「Publish」してください。

### Action のバージョン管理

ワークフロー内で利用する GitHub Actions は **commit SHA で固定**しています ([Security hardening for GitHub Actions](https://docs.github.com/actions/security-guides/security-hardening-for-github-actions#using-third-party-actions) 推奨)。バージョン更新は [`.github/dependabot.yml`](../.github/dependabot.yml) で毎週自動的に PR が作成されます。`# v2.9.1` のようなコメントで人間が読めるバージョン番号も併記しています。

### Branch / Tag Protection

- **`main` ブランチ**: 保護有効、CI green + PR 経由マージが必須
- **タグ (`v*`)**: 直接 push する権限を持つメンテナのみが作成可能となるよう、リポジトリ Settings で運用ルールを定めてください

## セキュリティ監査

```powershell
cargo install --locked cargo-audit
cargo audit
```

RustSec データベースに対する脆弱性チェックを実行します。

## トラブルシューティング

- **`cargo build` が `link.exe` エラーで失敗する**: C/C++ リンカが見つかっていません。前提環境を参照してください。
- **GUI が起動しない**: gpui のレンダリングバックエンド (blade-graphics) は Vulkan を要求します。GPU ドライバが Vulkan 1.1 以上に対応しているかご確認ください。
- **`--notify-probe` でイベントが出ない**: reparse point が絡むフォルダ (ジャンクション・シンボリックリンク経由のパス等) やネットワークドライブでは、`notify::recommended_watcher` がイベントを取得できないケースがあります。ローカルディスク上の実フォルダで再確認してください。
