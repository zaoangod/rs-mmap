//! 示例：顺序扫描文件统计行数，并用 `Advice::Sequential` 提示内核优化访问模式。
//!
//! mmap 按需调页，再大的文件也不会一次性读入内存。
//! Unix 上该提示对应 `madvise(2)`；Windows 上它是成功返回的无操作。
//!
//! 运行：`cargo run --example count_lines -- <文件路径>`（缺省统计 Cargo.toml）

use mmio::{Advice, Mmap};
use std::fs::File;
use std::time::Instant;

fn main() -> std::io::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| "Cargo.toml".into());
    let file = File::open(&path)?;

    // Safety: 映射存活期间不会截断该文件。
    let mmap = unsafe { Mmap::map(&file)? };

    // 这是最佳努力提示，不保证预读已经完成或避免后续缺页。
    mmap.advise(Advice::Sequential)?;

    let start = Instant::now();
    let lines = mmap.iter().filter(|&&b| b == b'\n').count();
    let elapsed = start.elapsed();

    println!("文件: {path}");
    println!("行数（换行符计数）: {lines}");
    println!("大小: {} 字节，扫描耗时: {elapsed:?}", mmap.len());
    Ok(())
}
