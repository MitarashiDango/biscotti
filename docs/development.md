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
├── about.toml / about.hbs   ─ THIRD-PARTY-LICENSES 生成設定 (リリース時に使用)
└── LICENSE                  ─ Apache License 2.0
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

## THIRD-PARTY-LICENSES

`THIRD-PARTY-LICENSES` (第三者ソフトウェアのライセンス集約) は**リポジトリには含まれません**。リリースワークフローがタグ push 時に `cargo about` で自動生成し、配布 ZIP に同梱します。

リポジトリには生成器の設定 (`about.toml`) とテンプレート (`about.hbs`) のみを置いています。

### ローカルで確認 / 生成したい場合

```powershell
cargo install --locked --features cli cargo-about
cargo about generate --all-features about.hbs -o THIRD-PARTY-LICENSES
```

生成されたファイルは `.gitignore` で除外されるため、誤ってコミットされる心配はありません。

`--all-features` を付けることで、`gui` feature 経由で入る `gpui` / `arboard` などの依存も網羅されます。

### ライセンスエラーが出た場合

`error: failed to satisfy license requirements` で停止した際は、未許可のライセンスが新たな依存に含まれています。`about.toml` の `accepted` 配列に追加可能なライセンスかを確認してから追記してください (Apache-2.0 と互換性のないライセンスを安易に追加しないこと)。

CI でも `cargo about generate` の成功/失敗のみを検証しているため、未許可ライセンスの混入は PR 時点で検知されます。

## CI / リリース自動化

### CI ([`.github/workflows/ci.yml`](../.github/workflows/ci.yml))

`main` への push および Pull Request 作成時に、Windows runner 上で以下を実行します。

- `cargo fmt --check`
- `cargo clippy --workspace --all-features -- -D warnings`
- `cargo test --workspace --features biscotti/gui`
- `cargo about generate` の試行 (未許可ライセンス混入の検知のみ。生成ファイルは破棄)

`main` ブランチは保護されており、CI が green でない PR はマージできません。

依存追加・更新時に未許可ライセンスが含まれていた場合は CI が失敗するので、`about.toml` の `accepted` を見直してください。

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
   - `cargo about generate` による `THIRD-PARTY-LICENSES` の生成
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
