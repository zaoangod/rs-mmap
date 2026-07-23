# mmio

`mmio` 是一个跨平台、**仅文件只读**的 Rust 内存映射库。

它提供与 [memmap2](https://github.com/RazrFalcon/memmap2-rs) 相同风格的 `Mm`、`MmOption` 与 `MmAsRawDesc`，但刻意只保留读取文件所需的最小 interface。

映射可直接解引用为 `&[u8]`，因此适合顺序扫描、大文件查找、零拷贝解析等场景。

它不是把整个文件读入内存；操作系统会在访问到对应页面时再调页。

## 范围与非目标

本 crate 只支持文件支撑的只读映射：

- 支持：整个文件或指定字节区间的 `Mm`、访问模式提示、Unix 内存锁定。
- 不支持：可写映射、匿名映射、写时复制、可执行映射、刷新、裸指针映射和 Linux `mremap`。

这不是 `memmap2` 的替代品；需要上述能力时应直接使用 `memmap2`。

保持范围狭窄的好处是：公开 interface 只描述“从文件取得只读字节切片”这一件事。

## 平台支持

<!--@formatter:off-->

| 平台                             | 创建映射                               | `populate`                      | `advise`                                   | `lock` / `unlock`         |
|----------------------------------|----------------------------------------|---------------------------------|--------------------------------------------|---------------------------|
| Unix（macOS、Linux、Android 等） | `mmap(2)`                              | Linux / Android：`MAP_POPULATE` | `madvise(2)`                               | `mlock(2)` / `munlock(2)` |
| Windows                          | `CreateFileMappingW` / `MapViewOfFile` | 无操作                          | 仅 `WillNeed` 调用 `PrefetchVirtualMemory` | 不提供                    |
| 其他目标                         | 返回 `ErrorKind::Unsupported`          | ---                             | ---                                        | ---                       |

<!--@formatter:on-->

Windows 上 `Normal`、`Random`、`Sequential` 与 `DontNeed` 都是成功返回的无操作；它们不能当作跨平台性能保证。

## 使用方法

```toml
[dependencies]
mmio = { path = "." }
```

### 映射整个文件

```rust
use mmio::Mm;
use std::fs::File;

fn main() -> std::io::Result<()> {
    let file = File::open("Cargo.toml")?;

    // 安全性：映射存活期间，文件内容不会被截断或并发修改。
    let mmap = unsafe { Mm::map(&file)? };
    assert_eq!(&mmap[..8], b"[package");

    // Mm 不借用 File；创建成功后可关闭或丢弃 file。
    drop(file);
    assert!(!mmap.is_empty());
    Ok(())
}
```

### 映射指定区间

```rust
use mmio::MmOption;
use std::fs::File;

fn main() -> std::io::Result<()> {
    let file = File::open("Cargo.toml")?;

    // 偏移不需要页对齐；内部会映射必要的前缀，并只暴露请求的 4 字节。
    // 安全性：18..22 在文件内，且映射期间文件不会变化。
    let mmap = unsafe { MmOption::new().offset(18).length(4).map(&file)? };
    assert_eq!(&mmap[..], b"mmio");
    Ok(())
}
```

### 零拷贝处理：`Mm` 就是 `&[u8]`

```rust
use mmio::Mm;
use std::fs::File;

fn main() -> std::io::Result<()> {
    let file = File::open("Cargo.toml")?;
    // 安全性：映射存活期间，文件内容不会被截断或并发修改。
    let mmap = unsafe { Mm::map(&file)? };

    // 解引用为 &[u8]，可以直接使用切片的全部方法做查找、解析。
    let lines = mmap.iter().filter(|&&b| b == b'\n').count();
    let version_pos = mmap
        .windows(9)
        .position(|w| w == b"version =");
    println!("{lines} lines, version at {version_pos:?}");
    Ok(())
}
```

### 访问提示与预填充

```rust
use mmio::{Advice, MmOption};
use std::fs::File;

fn main() -> std::io::Result<()> {
    let file = File::open("Cargo.toml")?;

    // populate：Linux/Android 上创建时预填充页面（MAP_POPULATE）。
    // 安全性：映射存活期间，文件内容不会被截断或并发修改。
    let mmap = unsafe { MmOption::new().populate().map(&file)? };

    // 顺序扫描前提示内核；这只是性能提示，不构成任何保证。
    mmap.advise(Advice::Sequential)?;
    // 也可以只提示映射内的某个子区间。
    mmap.advise_range(Advice::WillNeed, 0, 64.min(mmap.len()))?;
    Ok(())
}
```

除 `&File` 外，`map` 还接受实现 `MmAsRawDesc` 的来源：Unix 上为 `RawFd` 或
`&T: AsRawFd`，Windows 上为 `RawHandle` 或 `&T: AsRawHandle`。调用期间描述符必须有效；
创建成功后，`Mm` 不再借用来源对象。

### Unix：裸 fd 与锁页

```rust,no_run
use mmio::Mm;
use std::fs::File;
use std::os::unix::io::AsRawFd;

fn main() -> std::io::Result<()> {
    let file = File::open("Cargo.toml")?;

    // 直接使用裸 fd；描述符只需在 map 调用期间有效。
    // 安全性：映射存活期间，文件内容不会被截断或并发修改。
    let mmap = unsafe { Mm::map(file.as_raw_fd())? };

    // mlock / munlock：把整段映射锁定在 RAM 中，可能因额度或权限失败。
    mmap.lock()?;
    // ... 访问映射 ...
    mmap.unlock()?;
    Ok(())
}
```

更多可运行的完整示例见 [`examples/`](examples/)，可用 `cargo run --example <名称>` 运行。

## 映射长度、偏移与空文件

- 未调用 `MmOption::length` 时，库会在创建时读取文件长度，并将映射延伸到当时的文件末尾；`offset` 超过文件长度会返回错误。
- 调用 `length` 后，库会检查 `offset + len` 的整数溢出和 Rust 切片的 `isize::MAX` 上限，**不会**预先读取元数据确认请求区间仍在文件内。调用方必须保证 `offset..offset + len` 是有效文件区间；越过 EOF 的映射在不同平台可能失败，或在访问时触发致命错误，不能依赖其行为。
- `offset` 不必对齐。Unix 使用页大小，Windows 使用分配粒度（通常 64 KiB）向下对齐。对外只暴露用户请求的字节范围。
- 空文件和显式零长度是合法的；解引用结果为空切片。Unix 内部建立 1 字节映射作为 `mmap` 的零长度替代，Windows 使用不指向映射的非空标记。

## 安全性

`Mm::map` 和 `MmOption::map` 是 `unsafe`。调用者必须在**整个 `Mm` 生命周期**内维护下列不变量：

1. 被映射文件不可被截断；否则访问已不存在的页面会在 Unix 触发 `SIGBUS`，在 Windows 触发访问违例，通常直接终止进程。
2. 文件内容不可被其他映射或进程无同步地修改。共享映射会反映这些写入，读取方可能看到不一致内容；已经借出的切片也不再具有稳定的字节语义。
3. 显式配置 `offset` 与 `len` 时，区间必须位于文件当前长度内。
4. 若把 `&[u8]` 转换为类型化视图，调用者还要自行验证对齐、长度、字节表示和内容有效性。

文件的 Rust `File` 值或原始描述符不必在映射存活期间保留；这与“文件本身不得被改变”是两回事。关闭句柄不会解除映射，截断或改写同一文件仍然危险。

## 访问提示与锁页

`Mm::advise` 与 `Mm::advise_range` 只向操作系统提供性能提示：调用成功不表示页面已在内存中，也不表示之后不会发生缺页。

`advise_range` 的 `offset..offset + len` 必须完全位于映射内，否则返回 `ErrorKind::InvalidInput`。

`Advice::DontNeed` 在 Unix 上对应 `MADV_DONTNEED`，后续访问可能重新从文件取回页面。

它不是持久化、同步或缓存一致性机制；不要把它用于仍有借出切片或仍在读取的区间。

当前interface 将它作为普通 advice 暴露；如果库未来引入匿名或写时复制映射，该值需要拆分到显式 `unsafe` 的 advice interface，和 `memmap2` 保持一致。

Unix 上可用 `Mm::lock` / `unlock` 请求锁定整段映射。

它们可能因进程的锁页资源限制、权限或系统内存压力而失败；零长度映射是无操作。

## 实现结构

```text
src/lib.rs      公共 interface、长度校验、对齐参数 MappingShape
src/unix.rs     mmap / munmap / madvise / mlock adapter
src/windows.rs  CreateFileMappingW / MapViewOfFile / PrefetchVirtualMemory adapter
src/stub.rs     不支持目标的 ErrorKind::Unsupported adapter
tests/mmap.rs   平台无关集成测试；Unix 追加 fd 与 lock 测试
examples/       可运行的使用示例（cargo run --example <名称>）
```

公共层只依赖 `MmInner` 的 `map`、`pointer`、`length` 与 `advise` interface。

平台 adapter 自行选择其对齐粒度和系统调用，`MappingShape` 则集中验证映射范围的算术不变量。

## 验证

```bash
cargo fmt --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo check --target x86_64-pc-windows-gnu
cargo check --target wasm32-wasip2
```

最后两项只验证交叉编译，不能替代在 Windows 与其他 Unix 平台上的运行时测试。

## License

MIT，许可证全文见 [LICENSE](LICENSE)。
