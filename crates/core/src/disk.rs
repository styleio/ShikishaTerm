//! A working folder as somewhere files are read and written, on whichever
//! machine it is: this PC's disk, or a server or a MicroVM over the
//! connection the app already has. The loop builds one for the tab a panel
//! stands on (`runtime::folder_disk`), behind the same fence and the same
//! permissions the file list goes through; what reads and writes through it
//! -- decision records (`adr`), new Markdown files (`mdnew`) -- is written
//! once for both kinds of machine.
//!
//! Every path is from the working folder, with forward slashes.

pub trait Disk {
    /// The names of the files in a folder, `None` when there is no folder
    fn files(&self, dir: &str) -> Result<Option<Vec<String>>, String>;
    /// The names of the folders in a folder (empty when there is no folder)
    fn folders(&self, dir: &str) -> Result<Vec<String>, String>;
    /// A file's bytes, `None` when there is no file
    fn read(&self, path: &str) -> Result<Option<Vec<u8>>, String>;
    /// Write a file. `fresh` refuses one that is already there
    fn write(&self, path: &str, bytes: &[u8], fresh: bool) -> Result<(), String>;
    /// Make a folder and every folder above it
    fn make_dirs(&self, dir: &str) -> Result<(), String>;
}

/// A file or folder in `dir`, as one path
pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() { name.to_string() } else { format!("{dir}/{name}") }
}
