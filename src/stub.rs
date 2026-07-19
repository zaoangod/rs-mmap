//! 不支持平台的占位 adapter。
//!
//! 它让 crate 可以在未知目标上编译；所有映射构造都会返回
//! [`std::io::ErrorKind::Unsupported`]。由于 `MmapInner` 从不被构造，`Never` 使其余
//! interface 在类型层面保持不可达。

use std::fs::File;
use std::io;

use crate::Advice;

// https://doc.rust-lang.org/stable/std/primitive.never.html 的稳定替代。
enum Never {}

/// 内存映射的平台相关状态（占位，永远不能被实例化）。
pub struct MmapInner {
    never: Never,
}

impl MmapInner {
    /// 不支持的平台上创建映射总是失败，且不会访问来源文件。
    pub fn map(_: usize, _: &File, _: u64, _: bool) -> io::Result<MmapInner> {
        Err(io::ErrorKind::Unsupported.into())
    }

    pub fn pointer(&self) -> *const u8 {
        match self.never {}
    }

    pub fn length(&self) -> usize {
        match self.never {}
    }

    pub fn advise(&self, _: Advice, _: usize, _: usize) -> io::Result<()> {
        match self.never {}
    }
}

/// 返回文件长度。
pub fn file_len(file: &File) -> io::Result<u64> {
    Ok(file.metadata()?.len())
}
