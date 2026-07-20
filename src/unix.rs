//! Unix 平台的只读内存映射 adapter，基于 `mmap(2)`。
//!
//! 所有文件映射均为 `PROT_READ | MAP_SHARED`。公共层负责检查用户可见视图的范围；本
//! adapter 负责把它向下对齐到页边界，并在释放时恢复实际映射的基址和长度。

use std::fs::File;
use std::io::{Error, ErrorKind, Result};
use std::mem::ManuallyDrop;
use std::os::unix::io::{FromRawFd, RawFd};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{Advice, MappingShape};

/// `MAP_POPULATE` 仅存在于 Linux 和 Android；在其他平台上 populate 选项为空操作。
#[cfg(any(target_os = "android", target_os = "linux"))]
const MAP_POPULATE: libc::c_int = libc::MAP_POPULATE;

#[cfg(not(any(target_os = "android", target_os = "linux")))]
const MAP_POPULATE: libc::c_int = 0;

// Android 与 glibc 平台使用 64 位文件偏移接口。
#[cfg(any(target_os = "android", all(target_os = "linux", not(target_env = "musl"))))]
use libc::{mmap64 as mmap, off64_t as off_t};

#[cfg(not(any(target_os = "android", all(target_os = "linux", not(target_env = "musl")))))]
use libc::{mmap, off_t};

/// 返回操作系统的页大小（结果带静态缓存）。
fn page_size() -> usize {
    static PAGE_SIZE: AtomicUsize = AtomicUsize::new(0);
    match PAGE_SIZE.load(Ordering::Relaxed) {
        0 => {
            let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize };
            PAGE_SIZE.store(page_size, Ordering::Relaxed);
            page_size
        }
        page_size => page_size,
    }
}

/// 内存映射的平台相关状态。
pub struct MmInner {
    pointer: *mut libc::c_void,
    length: usize,
}

impl MmInner {
    /// 创建新的 `MmInner`，是 `mmap(2)` 的薄封装。
    ///
    /// `length` 是对外切片长度，不含为满足页对齐而额外映射的前缀。
    fn new(length: usize, prot: libc::c_int, flags: libc::c_int, file: RawFd, offset: u64) -> Result<MmInner> {
        let shape = MappingShape::new(offset, length, page_size())?;
        let map_offset: off_t = shape.map_offset.try_into().map_err(|_| Error::new(ErrorKind::InvalidInput, "memory map offset overflows off_t"))?;

        // 安全性：以空指针作为地址提示创建新映射总是安全的，
        // 不会修改任何既有映射或内存内容。
        let pointer = unsafe {
            mmap(
                ptr::null_mut(),
                shape.map_length.max(1) as libc::size_t,
                prot,
                flags,
                file,
                map_offset,
            )
        };

        if pointer == libc::MAP_FAILED {
            Err(Error::last_os_error())
        } else {
            // 安全性：指针与长度已校验，偏移量按要求计算。
            Ok(unsafe { Self::from_raw_parts(pointer, length, shape.view_offset) })
        }
    }

    /// 返回当前映射的 `(基址, 映射长度, 页内偏移)` 三元组。
    ///
    /// 注意“映射长度”是映射本身的长度，而非对外切片的长度。
    fn as_mm_parameter(&self) -> (*mut libc::c_void, usize, usize) {
        let offset: usize = self.pointer as usize % page_size();
        let length: usize = self.length + offset;

        // 存在两种内存布局：
        //
        // 1. 常规布局：mmap 基址 | 对齐前缀（被映射但忽略）| 用户请求的切片，
        //    可直接对应 (ptr, len, offset) 三元组。
        //
        // 2. 零长度映射：mmap 不支持零长度，实际创建了 1 字节的映射，
        //    即“零长度切片后接 1 个被映射的字节”，无法归入上面的形式，
        //    特殊处理为 (ptr, 1, 0)。
        if length == 0 {
            (self.pointer, 1, 0)
        } else {
            // 安全性：`MmInner` 保证 self.ptr 向下取整到页边界即真实映射基址，
            // 因此结果指针与 self.ptr 处于同一映射内。
            let pointer = unsafe { self.pointer.sub(offset) };
            (pointer, length, offset)
        }
    }

    /// 由原始组件构造 `MmInner`。
    ///
    /// # Safety
    ///
    /// - `pointer` 必须指向可用 `munmap(2)` 释放的映射起点（即由 `mmap(2)` 返回）；
    /// - 映射长度必须为 `len + offset`；若 `len + offset == 0` 则映射长度为 1；
    /// - `offset` 必须小于当前页大小。
    unsafe fn from_raw_parts(pointer: *mut libc::c_void, length: usize, offset: usize) -> Self {
        debug_assert_eq!(pointer as usize % page_size(), 0, "ptr not page-aligned");
        debug_assert!(offset < page_size(), "offset larger than page size");

        Self { pointer: unsafe { pointer.add(offset) }, length }
    }

    /// 创建从 `offset` 开始、长度为 `length` 字节的共享只读文件映射。
    ///
    /// `populate` 只在 Linux 与 Android 转换为 `MAP_POPULATE`；其他 Unix 目标保持相同
    /// interface 但不改变 `mmap` flags。
    pub fn map(length: usize, file: RawFd, offset: u64, populate: bool) -> Result<MmInner> {
        let populate = if populate {
            MAP_POPULATE
        } else {
            0
        };
        MmInner::new(
            length,
            libc::PROT_READ,
            libc::MAP_SHARED | populate,
            file,
            offset,
        )
    }

    /// 返回指向映射起始地址的指针。
    pub fn pointer(&self) -> *const u8 {
        self.pointer as *const u8
    }

    /// 返回映射的字节长度。
    pub fn length(&self) -> usize {
        self.length
    }

    /// 提示给定范围的预期访问模式（`madvise`）。
    ///
    /// 公共层和此处都验证完整区间，避免该内部 interface 被未来调用方错误使用。`madvise`
    /// 要求地址页对齐，因此调用范围会向下扩展到真实映射的前缀内。
    pub fn advise(&self, advice: Advice, offset: usize, length: usize) -> Result<()> {
        let end = offset.checked_add(length).ok_or_else(|| Error::from(ErrorKind::InvalidInput))?;
        if end > self.length {
            return Err(ErrorKind::InvalidInput.into());
        }
        if length == 0 {
            return Ok(());
        }
        let advice = match advice {
            Advice::Normal => libc::MADV_NORMAL,
            Advice::Random => libc::MADV_RANDOM,
            Advice::Sequential => libc::MADV_SEQUENTIAL,
            Advice::WillNeed => libc::MADV_WILLNEED,
            Advice::DontNeed => libc::MADV_DONTNEED,
        };
        // 从请求区间的实际地址向下对齐。不能从 `offset` 中减去对齐量：
        // 当映射本身不是页对齐时，对齐量可能大于 offset，造成下溢。
        let start = unsafe { (self.pointer as *mut u8).add(offset) };
        let alignment = start as usize % page_size();
        let aligned_start = unsafe { start.sub(alignment) };
        let aligned_length = length.checked_add(alignment).ok_or_else(|| Error::from(ErrorKind::InvalidInput))?;
        // 安全性：已验证请求区间；向下对齐后仍在映射实际持有的前缀内。
        let result = unsafe { libc::madvise(aligned_start.cast(), aligned_length, advice) };
        if result == 0 {
            Ok(())
        } else {
            Err(Error::last_os_error())
        }
    }

    /// 将整个映射锁定在 RAM 中（`mlock`）。
    ///
    /// 不负责绕过 `RLIMIT_MEMLOCK` 等系统限制；零长度映射无需系统调用。
    pub fn lock(&self) -> Result<()> {
        if self.length == 0 {
            return Ok(());
        }
        // 安全性：指针与长度对整个映射有效。
        if unsafe { libc::mlock(self.pointer, self.length) } != 0 {
            Err(Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// 解除整个映射的内存锁定（`munlock`）。
    ///
    /// 零长度映射无需系统调用。
    pub fn unlock(&self) -> Result<()> {
        if self.length == 0 {
            return Ok(());
        }
        // 安全性：指针与长度对整个映射有效。
        if unsafe { libc::munlock(self.pointer, self.length) } != 0 {
            Err(Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

// 安全性：MmInner 独占其映射资源，且对外只暴露不可变字节视图。
// 经由被映射文件发生的外部修改风险由 crate 根模块中 `unsafe` 构造函数的契约覆盖。
unsafe impl Send for MmInner {}
unsafe impl Sync for MmInner {}

impl Drop for MmInner {
    fn drop(&mut self) {
        let (pointer, length, _) = self.as_mm_parameter();
        // 忽略错误：对合法映射解除映射不会失败；而且在 `Drop` 中也没有合理的方式上报错误。
        unsafe { libc::munmap(pointer, length as libc::size_t) };
    }
}

/// 返回文件长度。不会消费或关闭传入的 fd。
///
/// `File::from_raw_fd` 只用于调用 `metadata`；`ManuallyDrop` 保证不会取得 fd 所有权。
pub fn get_file_length(file: RawFd) -> Result<u64> {
    // 安全性：不能因 drop 关闭传入的 fd，因此立即包进 ManuallyDrop。
    unsafe {
        let file: ManuallyDrop<File> = ManuallyDrop::new(File::from_raw_fd(file));
        Ok(file.metadata()?.len())
    }
}
