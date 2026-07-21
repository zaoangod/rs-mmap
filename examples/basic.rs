//! 基本用法：映射整个文件，并把它当作 `&[u8]` 做零拷贝读取。
//!
//! 运行：`cargo run --example basic`

use std::fs::{self, File};
use std::path::PathBuf;

use mmio::Mm;

fn main() -> std::io::Result<()> {
    // 准备一个临时文件作为被映射对象。
    let path: PathBuf = std::env::temp_dir().join(format!("mmio-example-basic-{}", std::process::id()));
    fs::write(&path, b"hello\nmmio\nworld\n")?;

    let file = File::open(&path)?;

    // 安全性：映射存活期间，文件内容不会被截断或并发修改。
    let mmap = unsafe { Mm::map(&file)? };

    // `Mm` 解引用为 `&[u8]`，切片方法全部可用：长度、查找、迭代等。
    println!("{} bytes", mmap.len());
    let lines = mmap.iter().filter(|&&b| b == b'\n').count();
    println!("{lines} lines");
    assert_eq!(&mmap[..5], b"hello");

    // `Mm` 不借用 `File`，映射创建成功后即可关闭文件。
    drop(file);
    assert!(!mmap.is_empty());

    // 先解除映射再删除文件（Windows 上映射存活时无法删除文件）。
    drop(mmap);
    fs::remove_file(&path)?;
    Ok(())
}
