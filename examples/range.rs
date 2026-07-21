//! 映射文件的指定字节区间：偏移量与长度。
//!
//! 运行：`cargo run --example range`

use std::fs::{self, File};
use std::path::PathBuf;

use mmio::MmOption;

fn main() -> std::io::Result<()> {
    let path: PathBuf = std::env::temp_dir().join(format!("mmio-example-range-{}", std::process::id()));
    fs::write(&path, b"0123456789abcdef")?;

    let file = File::open(&path)?;

    // 偏移量不需要页对齐；实现内部会向下对齐并多映射必要的前缀，
    // 对外只暴露请求的字节范围。
    // 安全性：4..10 在文件内，且映射期间文件不会变化。
    let window = unsafe { MmOption::new().offset(4).length(6).map(&file)? };
    assert_eq!(&window[..], b"456789");
    println!("window: {}", str::from_utf8(&window).unwrap());

    // 不调用 `length` 时，映射从偏移处延伸到创建时的文件末尾。
    // 安全性：10.. 在文件内，且映射期间文件不会变化。
    let tail = unsafe { MmOption::new().offset(10).map(&file)? };
    assert_eq!(&tail[..], b"abcdef");
    println!("tail: {}", str::from_utf8(&tail).unwrap());

    drop(window);
    drop(tail);
    fs::remove_file(&path)?;
    Ok(())
}
