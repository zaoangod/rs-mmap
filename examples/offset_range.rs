//! 示例：用 `MmapOptions` 映射文件的指定区间（偏移量无需页对齐）。
//!
//! 运行：`cargo run --example offset_range -- <文件路径> [偏移] [长度]`
//! 不指定长度时，映射从偏移处延伸至文件末尾。
//! 指定长度时，输入区间必须在文件内；不要把它用于探测或扩展文件末尾。

use mmio::MmapOptions;
use std::fs::File;

fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "Cargo.toml".into());
    let offset: u64 = args.next().map(|s| s.parse().expect("偏移必须是非负整数")).unwrap_or(0);
    let len: Option<usize> = args.next().map(|s| s.parse().expect("长度必须是非负整数"));

    let file = File::open(&path)?;

    let mut options = MmapOptions::new();
    options.offset(offset);
    if let Some(len) = len {
        options.len(len);
    }

    // Safety: 映射存活期间不会截断该文件。
    let mmap = unsafe { options.map(&file)? };

    println!("文件: {path}，偏移 {offset}，映射 {} 字节", mmap.len());
    println!("内容:\n{}", String::from_utf8_lossy(&mmap));
    Ok(())
}
