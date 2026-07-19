//! Windows 平台的只读内存映射 adapter，基于 `CreateFileMappingW` 和 `MapViewOfFile`。
//!
//! 公共层提供统一的用户视图；本 adapter 按系统分配粒度向下对齐文件偏移，只请求
//! `FILE_MAP_READ` 视图。Windows 没有 `madvise(2)`；仅预取有近似实现。

use std::ffi::c_void;
use std::fs::File;
use std::io::{Error, Result};
use std::mem::{self, ManuallyDrop};
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::ptr::{self, NonNull};

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Memory::{CreateFileMappingW, FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, PAGE_READONLY, PrefetchVirtualMemory, UnmapViewOfFile, WIN32_MEMORY_RANGE_ENTRY};
use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use crate::{Advice, MappingShape};

/// 返回操作系统的分配粒度（allocation granularity）。
///
/// `MapViewOfFile` 创建视图的偏移量必须是该值的整数倍（通常为 64 KiB），
/// 它一般比页大小更大。
fn allocation_granularity() -> usize {
    unsafe {
        let mut info = mem::zeroed::<SYSTEM_INFO>();
        GetSystemInfo(&mut info);
        info.dwAllocationGranularity as usize
    }
}

/// 内存映射的平台相关状态。
///
/// `ptr` 指向用户请求的视图起点，而不是 `MapViewOfFile` 返回的对齐基址；`Drop` 会恢复
/// 后者后再调用 `UnmapViewOfFile`。
pub struct MmapInner {
    ptr: *mut c_void,
    len: usize,
}

impl MmapInner {
    /// 创建从 `offset` 开始、长度为 `len` 字节的文件只读映射。
    ///
    /// Windows 要求视图偏移是分配粒度的整数倍，`MappingShape` 保留所需的前缀并只向
    /// caller 暴露请求的 `len` 字节。
    pub fn map(len: usize, handle: RawHandle, offset: u64, _populate: bool) -> Result<MmapInner> {
        if len == 0 {
            // `CreateFileMappingW` 会以 ERROR_FILE_INVALID 拒绝零长度映射；
            // 改为返回空映射。该指针虽悬空（dangling）但非空，并满足 u8 零长度切片的
            // 对齐要求，因此可安全地构造 `&[]`。
            return Ok(MmapInner::empty());
        }

        let shape = MappingShape::new(offset, len, allocation_granularity())?;
        // 文件映射对象必须覆盖整个视图范围。`MapViewOfFile` 再按对齐后的范围创建视图。
        let max_size = shape.map_end();

        unsafe {
            let mapping = CreateFileMappingW(
                handle as _,
                ptr::null(),
                PAGE_READONLY,
                (max_size >> 32) as u32,
                (max_size & 0xffff_ffff) as u32,
                ptr::null(),
            );
            if mapping.is_null() {
                return Err(Error::last_os_error());
            }

            let view = MapViewOfFile(
                mapping,
                FILE_MAP_READ,
                (shape.map_offset >> 32) as u32,
                (shape.map_offset & 0xffff_ffff) as u32,
                shape.map_length,
            );
            // 视图会持有映射对象（进而持有文件），因此映射对象句柄可以立即关闭。
            CloseHandle(mapping);

            if view.Value.is_null() {
                Err(Error::last_os_error())
            } else {
                Ok(MmapInner { ptr: view.Value.add(shape.view_offset), len })
            }
        }
    }

    fn empty() -> MmapInner {
        MmapInner { ptr: NonNull::<c_void>::dangling().as_ptr(), len: 0 }
    }

    /// 返回指向映射起始地址的指针。
    pub fn pointer(&self) -> *const u8 {
        self.ptr as *const u8
    }

    /// 返回映射的字节长度。
    pub fn length(&self) -> usize {
        self.len
    }

    /// 提示给定范围的预期访问模式。
    ///
    /// 只有 [`Advice::WillNeed`] 有 Windows 等价物
    /// （`PrefetchVirtualMemory`）；其余提示为空操作。预取是最佳努力行为，并不保证
    /// 页面已经驻留；用户范围已由公共层验证。
    pub fn advise(&self, advice: Advice, offset: usize, len: usize) -> Result<()> {
        match advice {
            Advice::WillNeed if len > 0 => {
                let range = WIN32_MEMORY_RANGE_ENTRY { VirtualAddress: unsafe { self.ptr.add(offset) }, NumberOfBytes: len };
                let result = unsafe { PrefetchVirtualMemory(GetCurrentProcess(), 1, &range, 0) };
                if result != 0 {
                    Ok(())
                } else {
                    Err(Error::last_os_error())
                }
            }
            _ => Ok(()),
        }
    }
}

// 安全性：MmapInner 独占其映射视图资源，且对外只暴露不可变字节视图。经由被映射文件
// 发生的外部修改风险由 crate 根模块中 `unsafe` 构造函数的契约覆盖。
unsafe impl Send for MmapInner {}
unsafe impl Sync for MmapInner {}

impl Drop for MmapInner {
    fn drop(&mut self) {
        if self.len == 0 {
            return;
        }
        // 恢复 `MapViewOfFile` 返回的按分配粒度对齐的基址。
        let alignment = self.ptr as usize % allocation_granularity();
        let base = MEMORY_MAPPED_VIEW_ADDRESS { Value: unsafe { self.ptr.sub(alignment) } };
        unsafe {
            let _ = UnmapViewOfFile(base);
        }
    }
}

/// 返回文件长度。不会消费或关闭传入的句柄。
///
/// `File::from_raw_handle` 只用于调用 `metadata`；`ManuallyDrop` 保证不会取得句柄所有权。
pub fn file_len(handle: RawHandle) -> Result<u64> {
    // 安全性：不能因 drop 关闭传入的句柄，因此立即包进 ManuallyDrop。
    unsafe {
        let file = ManuallyDrop::new(File::from_raw_handle(handle));
        Ok(file.metadata()?.len())
    }
}
