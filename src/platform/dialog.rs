use std::{ffi::c_void, path::PathBuf};

use windows::{
    Win32::{
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree},
        UI::Shell::{
            Common::COMDLG_FILTERSPEC, FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT, FileSaveDialog,
            IFileSaveDialog, SIGDN_FILESYSPATH,
        },
    },
    core::{HSTRING, PCWSTR},
};

pub fn choose_pdf_path(default_filename: &str) -> Result<Option<PathBuf>, String> {
    choose_save_path(
        default_filename,
        "pdf",
        "PDF document",
        "*.pdf",
        "Export ProofSnip Evidence",
    )
}

pub fn choose_png_path(default_filename: &str) -> Result<Option<PathBuf>, String> {
    choose_save_path(
        default_filename,
        "png",
        "PNG image",
        "*.png",
        "Save ProofSnip Capture",
    )
}

fn choose_save_path(
    default_filename: &str,
    extension: &str,
    filter_name: &str,
    pattern: &str,
    title: &str,
) -> Result<Option<PathBuf>, String> {
    // Safety: the UI thread is initialized as a COM STA during process setup. The dialog and
    // shell item are RAII COM objects, and the returned PWSTR is freed with CoTaskMemFree.
    unsafe {
        let dialog: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| format!("could not create save dialog: {error}"))?;
        let display_name = HSTRING::from(filter_name);
        let pattern = HSTRING::from(pattern);
        let filter = COMDLG_FILTERSPEC {
            pszName: PCWSTR(display_name.as_ptr()),
            pszSpec: PCWSTR(pattern.as_ptr()),
        };
        dialog
            .SetFileTypes(&[filter])
            .map_err(|error| format!("could not configure save dialog: {error}"))?;
        dialog
            .SetOptions(FOS_FORCEFILESYSTEM | FOS_OVERWRITEPROMPT)
            .map_err(|error| format!("could not configure save dialog options: {error}"))?;
        dialog
            .SetDefaultExtension(&HSTRING::from(extension))
            .map_err(|error| format!("could not set file extension: {error}"))?;
        dialog
            .SetFileName(&HSTRING::from(default_filename))
            .map_err(|error| format!("could not set default file name: {error}"))?;
        dialog
            .SetTitle(&HSTRING::from(title))
            .map_err(|error| format!("could not set save dialog title: {error}"))?;

        if let Err(error) = dialog.Show(None) {
            if error.code().0 as u32 == 0x8007_04c7 {
                return Ok(None);
            }
            return Err(format!("save dialog failed: {error}"));
        }

        let item = dialog
            .GetResult()
            .map_err(|error| format!("save dialog returned no file: {error}"))?;
        let path_ptr = item
            .GetDisplayName(SIGDN_FILESYSPATH)
            .map_err(|error| format!("could not read selected file path: {error}"))?;
        let path = path_ptr
            .to_string()
            .map(PathBuf::from)
            .map_err(|error| format!("selected file path was invalid: {error}"));
        CoTaskMemFree(Some(path_ptr.0.cast::<c_void>()));
        path.map(Some)
    }
}
