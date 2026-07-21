//! Unix 专属用法：直接用裸 fd 创建映射，并用 `mlock` 锁定页面。
//!
//! 运行：`cargo run --example unix_fd`（仅 Unix）

#[cfg(unix)]
use std::fs::{self, File};
#[cfg(unix)]
use std::os::unix::io::AsRawFd;
#[cfg(unix)]
use std::path::PathBuf;

#[cfg(unix)]
use mmio::Mm;

#[cfg(unix)]
fn main() -> std::io::Result<()> {
    let path: PathBuf = std::env::temp_dir().join(format!("mmio-example-unix-fd-{}", std::process::id()));
    fs::write(&path, b"mmio on unix")?;

    let file = File::open(&path)?;

    // `map` 接受任何实现 `MmAsRawDesc` 的来源，Unix 上包括裸 `RawFd`。
    // 描述符只需在调用期间有效；映射创建成功后即可关闭文件。
    // 安全性：映射存活期间，文件内容不会被截断或并发修改。
    let mmap = unsafe { Mm::map(file.as_raw_fd())? };
    drop(file);
    assert_eq!(&mmap[..4], b"mmio");

    // mlock：把整段映射锁定在 RAM 中，防止被换出。
    // 可能因进程的锁页额度、权限或内存压力失败；零长度映射是无操作。
    mmap.lock()?;
    println!("locked {} bytes in RAM", mmap.len());
    mmap.unlock()?;

    drop(mmap);
    fs::remove_file(&path)?;
    Ok(())
}

#[cfg(not(unix))]
fn main() {
    eprintln!("this example only runs on Unix");
}
