# Koyubi 開発ガイド

## 開発環境

### 推奨構成

```
自宅 (Ubuntu / Arch Linux on Distrobox)
├── koyubi-engine の開発・テスト（ネイティブ）
├── cargo-xwin によるクロスコンパイル（イテレーション用）
├── QEMU/KVM Windows VM（TSF 統合テスト用）
└── GitHub で管理

職場 (WSL2 + Windows)
├── 同じコードを pull
├── cargo build --target x86_64-pc-windows-msvc
├── 実機 Windows で TSF テスト
└── regsvr32 で登録 → 動作確認

CI (GitHub Actions)
├── Windows runner で MSVC ビルド
├── Inno Setup でインストーラー生成
└── GitHub Releases へアップロード
```

### 必要なツール

#### 共通

```bash
# Rust ツールチェイン
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Windows ターゲット追加
rustup target add x86_64-pc-windows-msvc
rustup target add i686-pc-windows-msvc  # 32bit (後で)
```

#### Linux 固有

```bash
# cargo-xwin (Linux → Windows クロスコンパイル)
cargo install cargo-xwin

# QEMU/KVM (Windows VM 用)
# Arch:
sudo pacman -S qemu-full virt-manager libvirt dnsmasq
# Ubuntu:
sudo apt install qemu-kvm virt-manager libvirt-daemon-system

sudo systemctl enable --now libvirtd
sudo usermod -aG libvirt $USER
```

#### Windows VM セットアップ

1. Windows 11 評価版をダウンロード:
   https://www.microsoft.com/en-us/evalcenter/evaluate-windows-11-enterprise

2. virt-manager で VM 作成:
   - CPU: host-passthrough
   - メモリ: 4GB+
   - ディスク: VirtIO
   - VirtIO ドライバ ISO をマウント

3. 共有フォルダ設定 (virtiofs or SMB)

## ビルドコマンド

### engine のみビルド・テスト（Linux ネイティブ）

```bash
# ビルド
cargo build -p koyubi-engine

# テスト
cargo test -p koyubi-engine

# 特定のテストを実行
cargo test -p koyubi-engine test_romaji
```

### Windows 向け DLL のクロスコンパイル（Linux 上）

```bash
cargo xwin build -p koyubi-tsf --target x86_64-pc-windows-msvc --release
# → target/x86_64-pc-windows-msvc/release/koyubi_tsf.dll
```

### Windows 上でのビルド（WSL2 / ネイティブ）

```bash
cargo build --release
# → target/release/koyubi_tsf.dll
```

### インストール/アンインストール（開発用・管理者権限必要）

```powershell
# 登録
regsvr32.exe "target\release\koyubi_tsf.dll"

# 解除（IME を使用中のアプリを閉じてから）
regsvr32.exe /u "target\release\koyubi_tsf.dll"
```

**注意**: IME がクラッシュすると Windows がフリーズする可能性があります。
開発中は VM 上でテストすることを強く推奨します。

## 開発の進め方

### 日常の開発サイクル

```
1. koyubi-engine のロジック変更（Linux 上）
2. cargo test -p koyubi-engine で動作確認
3. cargo xwin build で DLL 生成
4. 共有フォルダ経由で Windows VM に渡す
5. VM 上で regsvr32 /u → regsvr32 で再登録
6. メモ帳等で動作確認
7. コミット & プッシュ
```

### デバッグ

koyubi-tsf のデバッグはログ出力が基本。
OutputDebugString 経由で DebugView や VS のデバッガで確認できる。

```rust
// デバッグログマクロ
#[cfg(debug_assertions)]
macro_rules! debug_log {
    ($($arg:tt)*) => {
        unsafe {
            let msg = format!("[Koyubi] {}\0", format!($($arg)*));
            windows::Win32::System::Diagnostics::Debug::OutputDebugStringA(
                windows::core::PCSTR::from_raw(msg.as_ptr())
            );
        }
    };
}
```

### IME の再登録スクリプト

開発中に頻繁に使うので、スクリプト化しておくと便利：

```powershell
# scripts/reinstall.ps1
param([string]$DllPath = "target\release\koyubi_tsf.dll")

Write-Host "Unregistering..."
& regsvr32.exe /u /s $DllPath 2>$null

Start-Sleep -Seconds 1

Write-Host "Registering..."
& regsvr32.exe /s $DllPath

Write-Host "Done. Restart target applications to use new IME."
```

## 辞書ファイル

### SKK-JISYO.L の入手

SKK 辞書は GitHub で公開されています：
https://github.com/skk-dev/dict

```bash
# L 辞書（大辞書）をダウンロード
curl -L -o dict/SKK-JISYO.L \
  https://raw.githubusercontent.com/skk-dev/dict/master/SKK-JISYO.L
```

### エンコーディング

SKK-JISYO.L は EUC-JP エンコーディング。
`encoding_rs` クレートで読み込み時に UTF-8 に変換する。

## コーディング規約

- `unwrap()` は使わない（テストコード除く）。`Result` / `Option` を適切に処理
- TSF 層でのパニックは絶対に避ける（OS フリーズの原因になる）
- `unsafe` は TSF の COM インターフェース実装に限定し、最小限に
- koyubi-engine には `unsafe` を持ち込まない
