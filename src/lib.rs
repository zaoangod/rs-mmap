//! 跨平台的 Rust 内存映射文件只读 IO API。
//!
//! 本 crate 是对操作系统内存映射能力的薄抽象，仅提供文件的只读映射；
//! API 与实现对齐 [memmap2](https://github.com/RazrFalcon/memmap2-rs) 的只读子集。
//!
//! # 范围
//!
//! 只支持文件支撑的 [`Mm`]；不提供匿名映射、可写或写时复制映射、可执行映射、刷新、裸指针映射或重新映射。
//! 映射创建成功后不借用来源的 [`std::fs::File`]，但这不表示底层文件可以被截断或修改。
//!
//! # 安全性
//!
//! 文件内存映射存在一些类型系统无法防范的固有风险，因此构造函数 [`Mm::map`] / [`MmOption::map`] 是 `unsafe`：
//!
//! * 如果映射存活期间被映射的文件被截断（由本进程或其他进程），访问超出新文件末尾的页面会在 Unix 上触发 `SIGBUS`、在 Windows 上触发访问违例（access violation），通常导致进程崩溃。
//! * 其他映射或进程对同一文件的写入会不经任何同步地反映到本映射中；读取到中途被修改的数据可能产生不一致的视图。
//! * 显式设置 [`MmOption::length`] 时，调用者必须保证 `offset..offset + len` 位于文件当前范围内。库会检查整数溢出，但不会为显式长度再读取一次文件元数据。
//! * 只有当调用者校验过文件内容时，将映射内容视为 `&[u8]` 以外的类型化视图才是安全的。

#[cfg_attr(unix, path = "unix.rs")]
#[cfg_attr(windows, path = "windows.rs")]
#[cfg_attr(not(any(unix, windows)), path = "stub.rs")]
mod os;
use crate::os::{get_file_length, MmInner};

use std::fmt;
use std::slice;

#[cfg(not(any(unix, windows)))]
use std::fs::File;
use std::io::{Error, ErrorKind, Result};
use std::ops::Deref;

#[cfg(unix)]
use std::os::unix::io::{AsRawFd, RawFd};

#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, RawHandle};

/// 将调用者请求的视图转换为操作系统需要的对齐映射参数。
///
/// `map_offset` 是可传给 OS 的对齐文件偏移，`map_length` 包含因对齐而多映射的前缀，`view_offset` 则是应从 OS 返回的映射基址跳过的字节数。
/// 平台 adapter 只负责创建和释放映射；对齐与长度溢出规则集中在这里。
#[cfg(any(unix, windows))]
pub(crate) struct MappingShape {
    pub(crate) map_offset: u64,
    pub(crate) map_length: usize,
    pub(crate) view_offset: usize,
}

#[cfg(any(unix, windows))]
impl MappingShape {
    pub(crate) fn new(offset: u64, view_length: usize, granularity: usize) -> Result<Self> {
        debug_assert!(granularity > 0, "mapping granularity must not be zero");

        let view_offset: usize = (offset % granularity as u64) as usize;
        let map_offset: u64 = offset - view_offset as u64;
        let map_length: usize = view_length.checked_add(view_offset).ok_or_else(|| Error::new(ErrorKind::InvalidInput, "memory map length overflows usize"))?;
        let map_length_u64: u64 = u64::try_from(map_length).map_err(|_| Error::new(ErrorKind::InvalidInput, "memory map length overflows u64"))?;
        map_offset.checked_add(map_length_u64).ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidInput,
                "memory map offset + length overflows u64",
            )
        })?;

        Ok(Self { map_offset, map_length, view_offset })
    }

    #[cfg(windows)]
    pub(crate) fn map_end(&self) -> u64 {
        self.map_offset + self.map_length as u64
    }
}

/// 映射来源的底层描述符。
///
/// 在 Unix 上是 `RawFd`，在 Windows 上是 `RawHandle`，在其他平台上是 `&File`。
#[cfg(not(any(unix, windows)))]
pub struct MmRawDescriptor<'a>(&'a File);

/// 映射来源的底层描述符。
///
/// 在 Unix 上是 `RawFd`，在 Windows 上是 `RawHandle`，在其他平台上是 `&File`。
#[cfg(unix)]
pub struct MmRawDescriptor(RawFd);

/// 映射来源的底层描述符。
///
/// 在 Unix 上是 `RawFd`，在 Windows 上是 `RawHandle`，在其他平台上是 `&File`。
#[cfg(windows)]
pub struct MmRawDescriptor(RawHandle);

/// 可作为内存映射来源的类型。
///
/// 在 Unix 上为 `RawFd` 和所有 `&T: AsRawFd`（包括 `&File`）实现；
/// 在 Windows 上为 `RawHandle` 和所有 `&T: AsRawHandle` 实现；
/// 其他平台仅支持 `&File`。
///
/// 实现本 trait 的类型只在创建映射时被读取。调用 [`MmOption::map`] 时，描述符必须
/// 指向可读取的常规文件并在调用完成前保持有效；`Mm` 创建成功后不再借用它。
#[cfg(any(unix, windows))]
pub trait MmAsRawDesc {
    fn as_raw_desc(&self) -> MmRawDescriptor;
}

/// 可作为内存映射来源的类型。
///
/// 在 Unix 上为 `RawFd` 和所有 `&T: AsRawFd`（包括 `&File`）实现；
/// 在 Windows 上为 `RawHandle` 和所有 `&T: AsRawHandle` 实现；
/// 其他平台仅支持 `&File`。
///
/// 实现本 trait 的类型只在创建映射时被读取。调用 [`MmOption::map`] 时，描述符必须
/// 指向可读取的常规文件并在调用完成前保持有效；`Mm` 创建成功后不再借用它。
#[cfg(not(any(unix, windows)))]
pub trait MmAsRawDesc {
    fn as_raw_desc(&self) -> MmRawDescriptor<'_>;
}

#[cfg(not(any(unix, windows)))]
impl MmAsRawDesc for &File {
    fn as_raw_desc(&self) -> MmRawDescriptor<'_> {
        MmRawDescriptor(self)
    }
}

#[cfg(unix)]
impl MmAsRawDesc for RawFd {
    fn as_raw_desc(&self) -> MmRawDescriptor {
        MmRawDescriptor(*self)
    }
}

#[cfg(unix)]
impl<T> MmAsRawDesc for &T
where
    T: AsRawFd,
{
    fn as_raw_desc(&self) -> MmRawDescriptor {
        MmRawDescriptor(self.as_raw_fd())
    }
}

#[cfg(windows)]
impl MmAsRawDesc for RawHandle {
    fn as_raw_desc(&self) -> MmRawDescriptor {
        MmRawDescriptor(*self)
    }
}

#[cfg(windows)]
impl<T> MmAsRawDesc for &T
where
    T: AsRawHandle,
{
    fn as_raw_desc(&self) -> MmRawDescriptor {
        MmRawDescriptor(self.as_raw_handle())
    }
}

/// 向操作系统提示内存映射的预期访问模式。
///
/// 这是 `madvise(2)` 的跨平台等价物。在 Windows 上只有
/// [`Advice::WillNeed`] 有效果（调用 `PrefetchVirtualMemory`），
/// 其余提示均为空操作。调用成功不保证预取、逐出或改变任何缓存状态。
///
/// `DontNeed` 在 Unix 上可能使后续访问重新从文件取回页面；不要把它当作同步或
/// 一致性原语，也不要在仍有借出切片被使用的区间上调用它。若本 crate 未来支持匿名或
/// 写时复制映射，该变体应迁移到显式 `unsafe` 的 advice interface。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Advice {
    /// 不做特殊处理，即默认行为。
    Normal,
    /// 页面将被随机访问。
    Random,
    /// 页面将被顺序访问。
    Sequential,
    /// 页面即将被访问，建议预取。
    WillNeed,
    /// 页面近期内不会被访问；Unix 上对应 `MADV_DONTNEED`。
    DontNeed,
}

/// 内存映射的配置项，遵循 builder 模式。
///
/// 未配置 [`Self::length`] 时，映射长度在创建时从文件元数据推导；配置显式长度后，调用者
/// 负责保证区间仍在文件内。所有 setter 都只修改构建器，直到调用 [`Self::map`] 才会
/// 访问文件或调用操作系统。
///
/// ```no_run
/// use mmio::MmOption;
///
/// let mut options = MmOption::new();
/// options.offset(4096).length(1024).populate();
/// ```
#[derive(Clone, Debug, Default)]
pub struct MmOption {
    offset: u64,
    length: Option<usize>,
    populate: bool,
}

impl MmOption {
    /// 创建一组默认配置：偏移为 0、长度延伸至文件末尾、不预填充页面。
    pub fn new() -> MmOption {
        MmOption::default()
    }

    /// 配置映射起点相对文件开头的字节偏移量。
    ///
    /// 偏移量无需对齐；实现内部会将其向下对齐到所需的粒度，并相应调整返回的指针。
    /// 默认为 0。
    pub fn offset(&mut self, offset: u64) -> &mut Self {
        self.offset = offset;
        self
    }

    /// 配置映射的字节长度。
    ///
    /// 若不设置，映射将从偏移处延伸至创建时的文件末尾。若设置，库会验证长度与偏移的
    /// 算术边界，但不检查 `offset..offset + length` 是否仍在文件内；该不变量由调用者负责。
    pub fn length(&mut self, length: usize) -> &mut Self {
        self.length = Some(length);
        self
    }

    /// 配置映射在创建时预填充（populate）其所有页面。
    ///
    /// 在 Linux 和 Android 上对应 `MAP_POPULATE`，在其他平台上为空操作。它只是创建时的
    /// 最佳努力预取提示，不能保证之后的访问不会发生缺页。
    pub fn populate(&mut self) -> &mut Self {
        self.populate = true;
        self
    }

    /// 校验映射长度：Rust 切片的大小不能超过 `isize::MAX`。
    ///
    /// 在 64 位平台上不是问题，但 32 位平台上大于 2GB 的文件很常见，必须拦截。
    /// 能放入 `isize` 的无符号数必然能放入 `usize`。
    fn validate_length(len: u64) -> Result<usize> {
        if isize::try_from(len).is_err() {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "memory map length overflows isize",
            ));
        }
        Ok(len as usize)
    }

    /// 返回配置的长度；未配置时返回文件从偏移处到末尾的长度。
    fn get_length<T: MmAsRawDesc>(&self, file: &T) -> Result<usize> {
        let length: u64 = if let Some(v) = self.length {
            v as u64
        } else {
            let desc: MmRawDescriptor = file.as_raw_desc();
            let file_length: u64 = get_file_length(desc.0)?;

            if file_length < self.offset {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "memory map offset is larger than length",
                ));
            }

            file_length - self.offset
        };
        Self::validate_length(length)
    }

    /// 创建由文件支撑的只读内存映射。
    ///
    /// # Safety
    ///
    /// 调用者必须遵守 [crate 级文档](crate#安全性)中描述的各项约束；
    /// 特别是映射存活期间文件不得被截断或无同步地修改。显式长度还必须对应文件内的
    /// 有效区间。
    ///
    /// # Errors
    ///
    /// 当描述符无效、不可读取、偏移超出自动推导的文件长度、映射长度超过 Rust 切片
    /// 上限，或底层系统调用失败时返回错误。其他平台始终返回
    /// [`ErrorKind::Unsupported`]。
    ///
    /// # 示例
    ///
    /// ```
    /// use mmio::MmOption;
    /// use std::fs::File;
    ///
    /// # fn main() -> std::io::Result<()> {
    /// let file = File::open("Cargo.toml")?;
    /// // 安全性：映射存活期间文件不会被截断。
    /// let mapping = unsafe { MmOption::new().offset(18).length(4).map(&file)? };
    /// assert_eq!(&mapping[..], b"mmio");
    /// # Ok(())
    /// # }
    /// ```
    pub unsafe fn map<T: MmAsRawDesc>(&self, file: T) -> Result<Mm> {
        let desc = file.as_raw_desc();
        MmInner::map(self.get_length(&file)?, desc.0, self.offset, self.populate).map(|inner| Mm { inner })
    }
}

/// 只读文件内存映射。
///
/// 通过 [`Mm::map`] 或 [`MmOption::map`] 构造。可解引用为 `&[u8]`，
/// `len()`、`is_empty()`、`as_ptr()` 等方法均来自切片。`Mm` 不借用创建它的文件对象；
/// 关闭来源描述符不会解除映射。
pub struct Mm {
    inner: MmInner,
}

impl Mm {
    /// 创建映射整个文件的只读内存映射。
    ///
    /// 等价于 `MmOption::new().map(file)`。
    ///
    /// # Safety
    ///
    /// 调用者必须遵守 [crate 级文档](crate#安全性)中描述的各项约束；
    /// 特别是映射存活期间文件不得被截断或无同步地修改。
    ///
    /// # 示例
    ///
    /// ```
    /// use mmio::Mm;
    /// use std::fs::File;
    ///
    /// # fn main() -> std::io::Result<()> {
    /// let file = File::open("Cargo.toml")?;
    /// // 安全性：映射存活期间文件不会被截断。
    /// let mapping = unsafe { Mm::map(&file)? };
    /// assert_eq!(&mapping[..8], b"[package");
    /// # Ok(())
    /// # }
    /// ```
    pub unsafe fn map<T: MmAsRawDesc>(file: T) -> Result<Mm> {
        // Safety: 本函数与 `MmOption::map` 具有相同的安全约束，
        // 由调用者保证。
        unsafe { MmOption::new().map(file) }
    }

    /// 就整个映射向操作系统提示访问模式。
    ///
    /// 在 Unix 上对应 `madvise(2)`；在 Windows 上仅 [`Advice::WillNeed`] 有效。
    /// 这是性能提示，不提供缓存、一致性或预取完成保证。
    pub fn advise(&self, advice: Advice) -> Result<()> {
        self.advise_range(advice, 0, self.len())
    }

    /// 就映射的某个子区间向操作系统提示访问模式。
    ///
    /// `offset..offset + len` 必须完全落在映射范围内，否则返回
    /// [`ErrorKind::InvalidInput`]。操作系统仍可能拒绝某个 advice 或将其视为无操作。
    pub fn advise_range(&self, advice: Advice, offset: usize, length: usize) -> Result<()> {
        let end: usize = offset.checked_add(length).ok_or_else(|| Error::from(ErrorKind::InvalidInput))?;
        if end > self.len() {
            return Err(ErrorKind::InvalidInput.into());
        }
        self.inner.advise(advice, offset, length)
    }

    /// 将整个映射锁定在 RAM 中（`mlock`）。仅 Unix 支持。
    ///
    /// 受进程锁页额度与权限限制，可能返回 OS 错误。零长度映射是无操作。
    #[cfg(unix)]
    pub fn lock(&self) -> Result<()> {
        self.inner.lock()
    }

    /// 解除整个映射的内存锁定（`munlock`）。仅 Unix 支持。
    ///
    /// 对未锁定页面重复调用的具体语义由操作系统决定；零长度映射是无操作。
    #[cfg(unix)]
    pub fn unlock(&self) -> Result<()> {
        self.inner.unlock()
    }
}

impl Deref for Mm {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        // 安全性：`ptr` 在映射的整个生命周期内对 `len` 个字节有效，
        // 且映射是只读的。
        unsafe { slice::from_raw_parts(self.inner.pointer(), self.inner.length()) }
    }
}

impl AsRef<[u8]> for Mm {
    fn as_ref(&self) -> &[u8] {
        self.deref()
    }
}

impl fmt::Debug for Mm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mm").field("ptr", &self.as_ptr()).field("len", &self.len()).finish()
    }
}
