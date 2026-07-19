//! 基础示例：将整个文件映射为只读内存并读取。
//!
//! 运行：`cargo run --example read_file -- <文件路径>`（缺省读取 Cargo.toml）
//! 文件可在映射创建后关闭，但映射存活期间不得截断或无同步地改写它。

use mmio::Mmap;
use std::fs::File;

fn main() -> std::io::Result<()> {
    let path: String = std::env::args().nth(1).unwrap_or_else(|| "Cargo.toml".into());
    let file = File::open(&path)?;

    // Safety: 映射存活期间不会截断该文件。
    let mmap: Mmap = unsafe { Mmap::map(&file)? };

    println!("文件: {path}");
    println!("映射长度: {} 字节", mmap.len());

    if mmap.is_empty() {
        println!("空文件");
        return Ok(());
    }

    // 映射可直接当作 &[u8] 使用（Deref / AsRef<[u8]>）。
    let preview = &mmap[..mmap.len().min(64)];
    println!(
        "前 {} 字节:\n{}",
        preview.len(),
        String::from_utf8_lossy(preview)
    );
    Ok(())
}
