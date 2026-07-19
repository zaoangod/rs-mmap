//! 示例：在映射的文件中查找字节串，输出前若干个匹配位置的偏移。
//!
//! 大文件无需先整体读入内存：只有被访问到的页才会调入。
//! 模式按 UTF-8 命令行参数接收，并按其 UTF-8 字节序列匹配，而不是按字符边界匹配。
//!
//! 运行：`cargo run --example find_in_file -- <文件路径> <模式>`

use mmio::Mmap;
use std::fs::File;

const MAX_MATCHES: usize = 10;

fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let (path, pattern) = match (args.next(), args.next()) {
        (Some(path), Some(pattern)) => (path, pattern),
        _ => {
            eprintln!("用法: cargo run --example find_in_file -- <文件路径> <模式>");
            std::process::exit(2);
        }
    };

    let file = File::open(&path)?;
    // Safety: 映射存活期间不会截断该文件。
    let mmap = unsafe { Mmap::map(&file)? };

    let needle = pattern.as_bytes();
    let mut found = Vec::new();
    if !needle.is_empty() && mmap.len() >= needle.len() {
        for (i, window) in mmap.windows(needle.len()).enumerate() {
            if window == needle {
                found.push(i);
                if found.len() >= MAX_MATCHES {
                    break;
                }
            }
        }
    }

    if found.is_empty() {
        println!("在 {path} 中未找到 {pattern:?}");
    } else {
        println!(
            "在 {path} 中找到 {pattern:?}，前 {} 个偏移: {found:?}",
            found.len()
        );
    }
    Ok(())
}
