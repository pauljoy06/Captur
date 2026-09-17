# ProofSnip

ProofSnip is a Windows 11 snipping and evidence-export application written in Rust. The capture path is deliberately independent from PNG encoding, persistence, and PDF generation.

## Current implementation

- Resident global shortcuts:
  - `Ctrl+Shift+4`: region capture
  - `Ctrl+Shift+5`: capture the previous region again
  - `Ctrl+Shift+6`: show the evidence workspace
  - `Ctrl+Shift+7`: capture the monitor under the cursor
  - `Ctrl+Shift+8`: capture the active window
- Multi-adapter, multi-monitor DXGI Desktop Duplication capture.
- Native-pixel virtual-desktop stitching, including negative monitor coordinates and output rotation.
- Per-monitor-v2 DPI awareness and an egui overlay positioned in physical desktop coordinates.
- Immediate native `CF_DIBV5` clipboard transfer on mouse release. PNG compression is not involved.
- Escape cancellation, selection dimensions, dimmed outside area, handles, and compact copy confirmation.
- Background WIC PNG saving.
- In-memory evidence sessions with captions, Before/Action/After labels, reordering, removal, and Capture + Note.
- Background direct PDF generation with A4/landscape layouts, embedded DejaVu Sans, aspect-ratio preservation, and tall-capture pagination.
- Capture-path timing display for hotkey-to-overlay, desktop capture, crop, and mouse-release-to-clipboard.

ProofSnip has no networking, JIRA integration, browser runtime, database, Tokio runtime, or cross-platform capture abstraction.

## Architecture

```text
src/
├── app/                 orchestration and egui state machine
├── capture/
│   ├── dxgi.rs          D3D11/DXGI desktop duplication and stitching
│   └── region.rs        physical-pixel geometry
├── platform/
│   ├── hotkeys.rs       resident RegisterHotKey message thread
│   ├── clipboard.rs     CF_DIBV5 clipboard ownership
│   ├── dialog.rs        native save dialogs
│   └── windows.rs       DPI and native window positioning
├── encoding/wic.rs      isolated WIC PNG encoder
├── evidence/mod.rs      session model and JSON folder representation
├── export/pdf.rs        immutable model to direct PDF generation
└── ui/theme.rs          compact ProofSnip design tokens
```

The critical path is:

```text
hotkey → hide ProofSnip → DXGI snapshot → overlay
mouse release → native crop → CF_DIBV5 clipboard → confirmation
```

WIC compression, disk writes, session JSON, and PDF generation are never required before the clipboard is populated.

## Build from WSL2 with the Windows MSVC target

### Recommended: `cargo-xwin`

This builds the standard `x86_64-pc-windows-msvc` target from WSL while downloading the Microsoft CRT/SDK import libraries that are needed by the linker.

```bash
# One-time Rust setup
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustup toolchain install stable --profile minimal
rustup target add x86_64-pc-windows-msvc

# One-time LLVM + cargo-xwin setup. On Ubuntu, use apt if available:
sudo apt-get update
sudo apt-get install -y clang lld
cargo install cargo-xwin --locked

# Build ProofSnip
cd /home/paul/repos/captur
cargo xwin build --release --target x86_64-pc-windows-msvc
```

If LLVM was installed with Linuxbrew instead of apt:

```bash
export PATH="/home/linuxbrew/.linuxbrew/opt/llvm/bin:$HOME/.cargo/bin:$PATH"
cargo xwin build --release --target x86_64-pc-windows-msvc
```

The executable is:

```text
target/x86_64-pc-windows-msvc/release/proofsnip.exe
```

Launch it from WSL through Windows:

```bash
powershell.exe -NoProfile -Command \
  "& '$(wslpath -w target/x86_64-pc-windows-msvc/release/proofsnip.exe)'"
```

### Alternative: use Windows Cargo from WSL

Install these on Windows first:

1. Rustup with the stable MSVC toolchain from <https://rustup.rs/>.
2. Visual Studio 2022 Build Tools with **Desktop development with C++** and a Windows 11 SDK.

If the repository is on a Windows-mounted path such as `/mnt/c/dev/proofsnip`:

```bash
cd /mnt/c/dev/proofsnip
powershell.exe -NoProfile -Command \
  "cargo build --release --target x86_64-pc-windows-msvc"
powershell.exe -NoProfile -Command \
  "& '.\\target\\x86_64-pc-windows-msvc\\release\\proofsnip.exe'"
```

A repository under the WSL ext4 filesystem is exposed to Windows as `\\wsl.localhost\...`, but native Windows build tools can be less reliable or slower there. Moving the repository to `/mnt/c/dev/proofsnip` is the practical fallback, not changing to the GNU target.

## Windows-side prerequisites

At runtime, ProofSnip needs Windows 11 with a D3D11-capable display adapter. There are no servers or external runtime processes. The production executable uses system DXGI, D3D11, WIC, clipboard, shell dialog, and User32 APIs.

For the Windows-native build alternative, Visual Studio Build Tools and the Windows SDK must be installed from Windows. This cannot be usefully replaced by Linux packages. The `cargo-xwin` route avoids that Windows-side build prerequisite.

## Technical risks and current tradeoffs

- **DXGI lifecycle:** the first implementation recreates duplication objects for each snapshot. This is simpler and recovers naturally from display changes, but device creation is included in hotkey-to-overlay latency. The timing panel determines whether persistent per-adapter duplication objects are worth the added reset and device-loss handling.
- **Mixed-DPI overlay:** the process is per-monitor-v2 aware and all selection/crop geometry uses physical pixels, including negative virtual-desktop coordinates. A single HWND spanning monitors with different scale factors still requires real Windows hardware validation because winit/egui controls the rendering scale for that HWND.
- **Capture freshness:** ProofSnip hides its window and calls `DwmFlush` before acquiring a frame. Desktop Duplication can still return timeout or access-lost errors during display changes, secure-desktop transitions, or device resets. Those errors are reported and the next capture creates fresh DXGI state.
- **PDF memory:** export consumes an immutable session snapshot and raw BGRA images on a worker thread. This preserves screenshot quality and capture responsiveness, but a session containing many 4K screenshots can temporarily use substantial memory while the PDF is assembled.
- **WSL boundary:** compilation is validated in WSL with the MSVC target and `cargo-xwin`. Actual global-hotkey, clipboard, mixed-DPI, GPU, and paste behavior must be exercised in an interactive Windows desktop session.
- **eframe internals:** ProofSnip does not directly depend on or use `arboard`, `image`, or `png` for screenshot capture or encoding. The required eframe 0.36 native integration currently enables those crates transitively for its own clipboard/icon support and does not expose a feature to disable them. ProofSnip's capture clipboard remains the native `CF_DIBV5` implementation and WIC remains its only PNG encoder.

## Manual acceptance test

1. Launch `proofsnip.exe`.
2. Move focus to another Windows application.
3. Press `Ctrl+Shift+4`.
4. Drag a region and release the mouse.
5. Immediately press `Ctrl+V` in Paint, Teams, an editor, or another bitmap-capable application.
6. Press `Ctrl+Shift+5` and confirm the identical native-pixel region is copied.
7. Press `Ctrl+Shift+7` over each monitor, then `Ctrl+Shift+8` with a normal window active.
8. Press `Ctrl+Shift+6` to inspect the recorded timings.
9. Add several captures to evidence, caption and reorder them, then choose **Export PDF**.

The workspace performance card reports the first-order latency measurements. Validate mixed-DPI layouts with monitors at different Windows scale factors and with a monitor positioned left or above the primary display.

## Development checks

```bash
cargo fmt --check
cargo check --target x86_64-pc-windows-msvc
cargo test                 # pure model/layout tests when run on a host configuration that excludes Win32-only modules
cargo xwin build --release --target x86_64-pc-windows-msvc
```

PDF generation is directly implemented with `printpdf` and raw BGRA pixel data. The `png` crate is not a direct dependency. WIC remains the only screenshot PNG encoder.
