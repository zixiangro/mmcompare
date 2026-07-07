# 线程模型

## 原则

单线程心智模型为主。多线程代码**完全隔离**在 `app.rs` 中，仅用于图片解码。无 `Arc<Mutex<>>`，无全局线程池。

## 图片加载管线

```
用户操作（File>Open 或 拖拽）
  │
  ├─ 主线程：收集文件路径 (≤8)
  ├─ 主线程：每张图 spawn 一个子线程
  │    │
  │    ├─ 子线程 1：读文件 → decode_image_bytes() → tx.send()
  │    ├─ 子线程 2：读文件 → decode_image_bytes() → tx.send()
  │    └─ ...
  ├─ 主线程：drop(tx) 确保 rx 不阻塞
  │
  └─ 每帧 poll_loading():
       ├─ try_recv 收集已完成的解码结果
       ├─ 收齐后：load_texture() 上传 GPU
       └─ 更新 state.images → request_repaint()
```

## 子线程做什么

```rust
std::thread::spawn(move || {
    let bytes = std::fs::read(&path)?;           // 读文件
    let img = decode_image_bytes(&bytes)?;       // 解码 (image crate, CPU)
    tx.send(img).ok();                           // 发回主线程
});
```

子线程只用 `std::fs::read` + `image::load_from_memory`，都是纯 CPU 操作，无共享状态。

## 通信

唯一跨线程通道：`std::sync::mpsc::channel()`

- 主线程持有 `Receiver`
- 每个子线程持有克隆的 `Sender`
- `drop(tx)` 后 rx 在所有 sender 关闭时自然结束

## 两种加载模式

| 触发 | 方法 | append 参数 |
|---|---|---|
| File → Open | `start_open()` | `false` — 替换已有图片 |
| 拖拽文件 | `poll_drops()` | `true` — 追加到末尾 |

拖拽时检查 `8 - images.len() - loading_total` 剩余名额，超出的忽略。
