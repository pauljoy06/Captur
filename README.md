# Captur

Captur is a Windows 11 snipping and evidence-export application written in Rust. The capture path is deliberately independent from PNG encoding, persistence, and PDF generation.

## Current implementation

- Resident global shortcuts:
  - `Ctrl+Alt+S`: region capture
  - `Ctrl+Alt+R`: capture the previous region again
  - `Ctrl+Alt+W`: show or hide the evidence workspace
  - `Ctrl+Alt+M`: capture the monitor under the cursor
  - `Ctrl+Alt+A`: capture the active window
  - `Ctrl+Alt+N`: region capture followed by the compact evidence-note prompt
- Multi-adapter, multi-monitor DXGI Desktop Duplication capture.
- Native-pixel virtual-desktop stitching, including negative monitor coordinates and output rotation.
- Per-monitor-v2 DPI awareness and an egui overlay positioned in physical desktop coordinates.
- Immediate native `CF_DIBV5` clipboard transfer on mouse release. PNG compression is not involved.
- Escape cancellation, selection dimensions, dimmed outside area, handles, and compact copy confirmation.
- Background WIC PNG saving.
- In-memory evidence sessions with captions, Before/Action/After labels, reordering, removal, and Capture + Note.
- Optional annotation workspace with Arrow, Rectangle, Highlight, Text, Blur/redact, and sequential number markers. The primary tools are keyboard accessible with `A`, `R`, `H`, `T`, `B`, `1`, `C`, `Enter`, and `Escape`.
- Lightweight always-on-top pinned screenshot viewport for visual comparison.
- Notification-area lifecycle with **Show Captur** and **Exit** actions. Closing the workspace keeps the resident capture process running.
- One embedded multi-resolution Captur icon shared by the executable, taskbar window, and notification area.
- Optional per-user Windows startup registration through `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. Login startup uses `--background`, so only the resident hotkeys and notification-area icon start initially.
- Background direct PDF generation with A4/landscape layouts, embedded DejaVu Sans, aspect-ratio preservation, and tall-capture pagination.
- Capture-path timing display for hotkey-to-overlay, desktop capture, crop, and mouse-release-to-clipboard.

Captur has no networking, JIRA integration, browser runtime, database, Tokio runtime, or cross-platform capture abstraction.

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
│   ├── startup.rs       current-user Windows startup registration
│   ├── tray.rs          notification-area icon and native message loop
│   └── windows.rs       DPI and native window positioning
├── annotations/mod.rs   annotation model and BGRA software rasterizer
├── encoding/wic.rs      isolated WIC PNG encoder
├── evidence/mod.rs      session model and JSON folder representation
├── export/pdf.rs        immutable model to direct PDF generation
└── ui/
    ├── theme.rs         Captur design tokens and dark visual system
    └── components.rs    reusable cards, buttons, badges, and status surfaces
assets/
├── captur.png           taskbar/window icon source
├── captur.ico           multi-resolution Windows icon
└── captur.rc            native executable resource declaration
build.rs                 dependency-free Windows resource compilation
```

The critical path is:

```text
hotkey → hide Captur → DXGI snapshot → overlay
mouse release → native crop → CF_DIBV5 clipboard → confirmation
```

WIC compression, disk writes, session JSON, and PDF generation are never required before the clipboard is populated.

DXGI devices and Desktop Duplication objects are initialized once and retained on the UI thread. Each output keeps its latest completed frame so an unchanged desktop does not turn a short duplication timeout into a failed snip. Access-loss and other duplication failures discard the engine and rebuild it once before the capture is reported as failed.

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

# Build Captur
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
target/x86_64-pc-windows-msvc/release/captur.exe
```

Launch it from WSL through Windows:

```bash
powershell.exe -NoProfile -Command \
  "& '$(wslpath -w target/x86_64-pc-windows-msvc/release/captur.exe)'"
```

### Alternative: use Windows Cargo from WSL

Install these on Windows first:

1. Rustup with the stable MSVC toolchain from <https://rustup.rs/>.
2. Visual Studio 2022 Build Tools with **Desktop development with C++** and a Windows 11 SDK.

If the repository is on a Windows-mounted path such as `/mnt/c/dev/captur`:

```bash
cd /mnt/c/dev/captur
powershell.exe -NoProfile -Command \
  "cargo build --release --target x86_64-pc-windows-msvc"
powershell.exe -NoProfile -Command \
  "& '.\\target\\x86_64-pc-windows-msvc\\release\\captur.exe'"
```

A repository under the WSL ext4 filesystem is exposed to Windows as `\\wsl.localhost\...`, but native Windows build tools can be less reliable or slower there. Moving the repository to `/mnt/c/dev/captur` is the practical fallback, not changing to the GNU target.

## Windows-side prerequisites

At runtime, Captur needs Windows 11 with a D3D11-capable display adapter. There are no servers or external runtime processes. The production executable uses system DXGI, D3D11, WIC, clipboard, shell dialog, and User32 APIs.

For the Windows-native build alternative, Visual Studio Build Tools and the Windows SDK must be installed from Windows. The SDK's `rc.exe` embeds the application icon. This cannot be usefully replaced by Linux packages. The `cargo-xwin` route uses `llvm-rc` from the LLVM installation instead.

## Technical risks and current tradeoffs

- **DXGI lifecycle:** Captur retains one duplication object per attached output to keep repeated captures responsive. Any acquisition failure rebuilds the complete engine once, which also picks up monitor and adapter changes. Display changes and secure-desktop transitions can still make that individual capture fail after the retry.
- **Mixed-DPI overlay:** the process is per-monitor-v2 aware and all selection/crop geometry uses physical pixels, including negative virtual-desktop coordinates. A single HWND spanning monitors with different scale factors still requires real Windows hardware validation because winit/egui controls the rendering scale for that HWND.
- **Capture freshness:** Captur hides its window and calls `DwmFlush` before acquiring a frame. Desktop Duplication can still return timeout or access-lost errors during display changes, secure-desktop transitions, or device resets. Those errors are reported and the next capture creates fresh DXGI state.
- **PDF memory:** export consumes an immutable session snapshot and raw BGRA images on a worker thread. This preserves screenshot quality and capture responsiveness, but a session containing many 4K screenshots can temporarily use substantial memory while the PDF is assembled.
- **WSL boundary:** compilation is validated in WSL with the MSVC target and `cargo-xwin`. Global hotkeys, DXGI, clipboard, native dialogs, tray behavior, and PDF export have also been exercised by the Windows acceptance scripts. The latest validation covered a 4480×1440 two-monitor virtual desktop. A monitor positioned left or above the primary display and a mixed-scale monitor pair remain manual hardware checks.
- **Secondary egui viewports:** pinned screenshots use egui's immediate native viewport support. Always-on-top behavior, resizing, and close handling should be checked against the installed Windows graphics driver and desktop configuration.
- **Annotation cost:** annotations are previewed as egui vector shapes, then rasterized into a BGRA frame only when copied or completed. Text rasterization uses isolated GDI calls through windows-rs. This work is outside the snipping critical path.
- **eframe internals:** Captur does not directly depend on or use `arboard`, `image`, or `png` for screenshot capture or encoding. The required eframe 0.36 native integration currently enables those crates transitively for its own clipboard/icon support and does not expose a feature to disable them. Captur's capture clipboard remains the native `CF_DIBV5` implementation and WIC remains its only PNG encoder.

## Manual acceptance test

1. Launch `captur.exe`.
2. Move focus to another Windows application.
3. Press `Ctrl+Alt+S`.
4. Drag a region and release the mouse.
5. Immediately press `Ctrl+V` in Paint, Teams, an editor, or another bitmap-capable application.
6. Press `Ctrl+Alt+R` and confirm the identical native-pixel region is copied.
7. Press `Ctrl+Alt+M` over each monitor, then `Ctrl+Alt+A` with a normal window active.
8. Press `Ctrl+Alt+W` to show the workspace and inspect the recorded timings. Press it again to hide the workspace.
9. Choose **Annotate**, exercise `A`, `R`, `H`, `T`, `B`, and `1`, then press `Enter`. Confirm the annotated result is immediately pasteable.
10. Choose **Pin latest**, switch to another application, and confirm the capture remains above normal windows without blocking input.
11. Close the workspace, confirm Captur remains in the notification area, reopen it from the icon, and use the icon's **Exit** command when finished.
12. Toggle **Start Captur when I sign in to Windows**, verify the current-user Run value, then toggle it off if startup is not desired.
13. Add several captures to evidence, caption and reorder them, then choose **Export PDF**. Check one, multiple, portrait, wide, 4K, and tall screenshots in the generated document.

The workspace performance card reports the first-order latency measurements. Validate mixed-DPI layouts with monitors at different Windows scale factors and with a monitor positioned left or above the primary display.

## Observed Windows acceptance results

The packaged release executable was exercised on Windows 11 through its real global hotkeys, native mouse input, DXGI capture, `CF_DIBV5` clipboard path, egui workspace, COM save dialog, tray window, and current-user Run key. The latest measured run observed:

| Workflow | Observed result |
| --- | ---: |
| Cold process start to resident window | 526 ms |
| Region hotkey to full 4480×1440 two-monitor overlay | 459 ms |
| Region mouse release to clipboard bitmap | 84 ms |
| 320×180 physical drag to exact 320×180 captured bitmap | matched |
| Same-region hotkey to clipboard | 226 ms |
| Full-monitor hotkey to clipboard | 478 ms |
| Active-window hotkey to clipboard | 420 ms |
| Escape to hidden resident state | 269 ms |
| Two-capture evidence export after save-dialog action | 3536 ms |

The final evidence run produced a valid 727,136-byte PDF from two 2560×1440 captures. Extracted document text confirmed the session title, reordered `After`/`Before` labels, both captions, and generated timestamps. The UX run confirmed that an annotation changed screenshot pixels without changing its 2560×1440 dimensions, the pin viewport was visible and topmost, the startup checkbox changed and restored the Run value, closing kept the process resident, the tray reopened the workspace, and tray Exit stopped it. Every acceptance script restored the clipboard, startup value, and resident process state it changed.

These are development measurements, not performance guarantees. The automated acceptance scripts use window-relative coordinates so they can follow Captur's DPI-aware workspace sizing. Run the scripts only in a disposable interactive desktop session because they temporarily move the cursor, use the clipboard, open windows, and toggle Captur's own startup value. Each script restores the state it changes in a `finally` block.

From WSL, copy the release executable and scripts to a Windows-local temporary folder before running them. This avoids occasional PowerShell/COM stalls when a script itself is loaded from a `\\wsl.localhost` UNC path:

```bash
mkdir -p /mnt/c/Temp/CapturAcceptance
cp target/x86_64-pc-windows-msvc/release/captur.exe \
  /mnt/c/Temp/CapturAcceptance/
cp scripts/windows-*-acceptance.ps1 /mnt/c/Temp/CapturAcceptance/

powershell.exe -NoProfile -ExecutionPolicy Bypass -File \
  'C:\Temp\CapturAcceptance\windows-capture-acceptance.ps1' \
  -ExePath 'C:\Temp\CapturAcceptance\captur.exe' \
  -ResultPath 'C:\Temp\CapturAcceptance\capture.json'

powershell.exe -NoProfile -ExecutionPolicy Bypass -File \
  'C:\Temp\CapturAcceptance\windows-evidence-acceptance.ps1' \
  -ExePath 'C:\Temp\CapturAcceptance\captur.exe' \
  -PdfPath 'C:\Temp\CapturAcceptance\evidence.pdf' \
  -ResultPath 'C:\Temp\CapturAcceptance\evidence.json' \
  -ScreenshotPath 'C:\Temp\CapturAcceptance\evidence.png'

powershell.exe -NoProfile -ExecutionPolicy Bypass -File \
  'C:\Temp\CapturAcceptance\windows-ux-acceptance.ps1' \
  -ExePath 'C:\Temp\CapturAcceptance\captur.exe' \
  -ResultPath 'C:\Temp\CapturAcceptance\ux.json'
```

## Development checks

```bash
cargo fmt --check
cargo check --target x86_64-pc-windows-msvc
cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings
cargo test                 # pure model/layout tests when run on a host configuration that excludes Win32-only modules
cargo xwin build --release --target x86_64-pc-windows-msvc
```

PDF generation is directly implemented with `printpdf` and raw BGRA pixel data. The `png` crate is not a direct dependency. WIC remains the only screenshot PNG encoder.
