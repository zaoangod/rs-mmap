//! 访问模式提示（`madvise`）与创建时预填充（populate）。
//!
//! 运行：`cargo run --example advice`

use std::fs::{self, File};
use std::path::PathBuf;

use mmio::{Advice, MmOption};

fn main() -> std::io::Result<()> {
    let path: PathBuf = std::env::temp_dir().join(format!("mmio-example-advice-{}", std::process::id()));
    fs::write(&path, vec![0u8; 64 * 1024])?;

    let file = File::open(&path)?;

    // populate：在 Linux 和 Android 上对应 `MAP_POPULATE`，创建映射时
    // 尽力预填充全部页面；在其他平台上是无操作。
    // 安全性：映射存活期间，文件内容不会被截断或并发修改。
    let mmap = unsafe { MmOption::new().populate().map(&file)? };

    // 准备顺序扫描整个映射，提示内核做预读。
    // 这只是性能提示：成功返回不代表页面已在内存中。
    mmap.advise(Advice::Sequential)?;

    // 也可以只提示映射内的子区间；区间必须完全落在映射内，
    // 否则返回 `ErrorKind::InvalidInput`。
    mmap.advise_range(Advice::WillNeed, 0, 4096)?;
    assert!(mmap.advise_range(Advice::WillNeed, mmap.len() - 1, 2).is_err());

    // 扫描一遍；`DontNeed` 提示这些页面近期不再需要（Unix: MADV_DONTNEED）。
    let sum: u64 = mmap.iter().map(|&b| u64::from(b)).sum();
    println!("sum = {sum}");
    mmap.advise(Advice::DontNeed)?;

    drop(mmap);
    fs::remove_file(&path)?;
    Ok(())
}
