//! `mmio` 的集成测试，覆盖只读映射的各个方面以及边界情况。
//!
//! 仅使用标准库：临时目录由 [`TestDir`] 在系统临时目录下创建，drop 时自动清理。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use mmio::{Advice, Mm, MmOption};

/// 测试专用临时目录：在系统临时目录下创建唯一子目录，drop 时递归删除。
///
/// 目录名包含进程 id 与递增序号，并发测试之间、并发的测试进程之间都不会冲突。
struct TestDir(PathBuf);

impl TestDir {
    fn new(test: &str) -> TestDir {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("mmio-test-{}-{test}-{id}", std::process::id()));
        fs::create_dir(&path).unwrap();
        TestDir(path)
    }

    /// 创建一个长度为 `len`、内容全为零的新文件。
    fn create_file(&self, name: &str, len: u64) -> File {
        let path = self.0.join(name);
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path).unwrap();
        file.set_len(len).unwrap();
        file
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// 只读映射：长度正确，内容与文件一致（全零）。
#[test]
fn map_read() {
    let dir = TestDir::new("read");
    let file = dir.create_file("read", 128);

    let mmap = unsafe { Mm::map(&file).unwrap() };
    assert_eq!(mmap.len(), 128);
    assert!(!mmap.is_empty());
    assert!(mmap.iter().all(|&b| b == 0));
}

/// 空文件可以映射为零长度映射，并解引用为空切片。
#[test]
fn map_empty_file() {
    let dir = TestDir::new("empty");
    let file = dir.create_file("empty", 0);

    let mmap = unsafe { Mm::map(&file).unwrap() };
    assert_eq!(mmap.len(), 0);
    assert!(mmap.is_empty());
    assert_eq!(&mmap[..], &[]);
    // 零长度映射的指针仍按字对齐。
    assert_eq!(mmap.as_ptr().align_offset(std::mem::size_of::<usize>()), 0);

    #[cfg(unix)]
    {
        mmap.lock().unwrap();
        mmap.unlock().unwrap();
    }
}

/// 带偏移量和长度的映射：视图应对应文件的指定区间。
#[test]
fn map_with_offset_and_len() {
    let dir = TestDir::new("offset");
    let file = dir.create_file("offset", 256);
    let contents: Vec<u8> = (0..=255).collect();
    (&file).write_all(&contents).unwrap();
    file.sync_all().unwrap();

    let mmap = unsafe { MmOption::new().offset(33).length(9).map(&file).unwrap() };
    assert_eq!(mmap.len(), 9);
    assert_eq!(&mmap[..], &contents[33..42]);
    // 非页对齐映射的建议范围从 0 开始时仍应正确向下对齐。
    mmap.advise_range(Advice::Sequential, 0, 9).unwrap();
    // 两个参数各自落在范围内还不够，整个区间也必须落在映射内。
    assert_eq!(
        mmap.advise_range(Advice::Sequential, 8, 2).unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput
    );

    // 不指定长度时，映射从偏移处延伸至文件末尾。
    let mmap = unsafe { MmOption::new().offset(200).map(&file).unwrap() };
    assert_eq!(mmap.len(), 56);
    assert_eq!(&mmap[..], &contents[200..]);
}

/// 偏移量超出文件长度时应返回错误。
#[test]
fn map_offset_beyond_file_errors() {
    let dir = TestDir::new("bounds");
    let file = dir.create_file("bounds", 16);

    let result = unsafe { MmOption::new().offset(17).map(&file) };
    assert!(result.is_err());
}

/// 偏移量与显式长度之和溢出时应返回错误，而不是传给平台 adapter。
#[test]
fn map_offset_and_len_overflow_errors() {
    let dir = TestDir::new("overflow");
    let file = dir.create_file("overflow", 1);

    let result = unsafe { MmOption::new().offset(u64::MAX).length(1).map(&file) };
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::InvalidInput);
}

/// 可以直接用裸 fd 创建映射（`MmAsRawDesc`）。
#[cfg(unix)]
#[test]
fn map_fd() {
    let dir = TestDir::new("fd");
    let file = dir.create_file("fd", 128);

    let mmap = unsafe { Mm::map(file.as_raw_fd()).unwrap() };
    assert_eq!(mmap.len(), 128);
    assert!(mmap.iter().all(|&b| b == 0));
}

/// 大于 4 GiB 的偏移量：推断长度与显式长度都正确。
#[test]
fn map_big_offset() {
    let dir = TestDir::new("big_offset");
    let offset = u64::from(u32::MAX) + 2;
    let len = 5432_u64;
    let file = dir.create_file("big_offset", offset + len);

    // 推断长度。
    let mmap = unsafe { MmOption::new().offset(offset).map(&file).unwrap() };
    assert_eq!(mmap.len(), len as usize);

    // 显式长度。
    let mmap = unsafe { MmOption::new().offset(offset).length(len as usize).map(&file).unwrap() };
    assert_eq!(mmap.len(), len as usize);
}

/// 各种访问模式提示（`madvise`）在所有平台上都应成功返回；
/// 越界范围应返回错误。
#[test]
fn advise() {
    let dir = TestDir::new("advise");
    let file = dir.create_file("advise", 128);

    let mmap = unsafe { Mm::map(&file).unwrap() };
    mmap.advise(Advice::Normal).unwrap();
    mmap.advise(Advice::Random).unwrap();
    mmap.advise(Advice::Sequential).unwrap();
    mmap.advise(Advice::WillNeed).unwrap();
    mmap.advise(Advice::DontNeed).unwrap();
    mmap.advise_range(Advice::Sequential, 0, 64).unwrap();

    // 越界范围返回 InvalidInput。
    mmap.advise_range(Advice::Sequential, 200, 10).unwrap_err();
    mmap.advise_range(Advice::Sequential, 0, 200).unwrap_err();
}

/// `mlock` / `munlock`：加锁与解锁都应成功，重复调用亦然。
#[cfg(unix)]
#[test]
fn lock_unlock() {
    let dir = TestDir::new("lock");
    let file = dir.create_file("lock", 128);

    let mmap = unsafe { Mm::map(&file).unwrap() };
    mmap.lock().unwrap();
    mmap.lock().unwrap();
    mmap.unlock().unwrap();
    mmap.unlock().unwrap();
}

/// 类型约束与人格化接口：Send + Sync、Deref、AsRef、Debug。
#[test]
fn traits() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Mm>();

    let dir = TestDir::new("traits");
    let file = dir.create_file("traits", 16);
    (&file).write_all(b"test").unwrap();
    file.sync_all().unwrap();

    let mmap = unsafe { Mm::map(&file).unwrap() };
    let slice: &[u8] = mmap.as_ref();
    assert_eq!(&slice[..4], b"test");
    assert_eq!(&mmap[..4], b"test"); // 通过 Deref 访问

    // Debug 输出应以类型名 `Mm` 开头。
    let debug = format!("{:?}", mmap);
    assert!(debug.starts_with("Mm {"), "unexpected: {debug}");
    let _ = format!("{:?}", MmOption::new().offset(1).length(2));
}
