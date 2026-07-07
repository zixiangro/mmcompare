# 线程模型

## 原则

单线程心智模型为主。多线程代码**完全隔离**在 `app.rs` 中，仅用于图片解码。无 `Arc<Mutex<>>`，无全局线程池。

## 图片加载管线

```
用户拖拽文件到窗口
  │
  ├─ 主线程：poll_drops() 取 ctx.input().raw.dropped_files
  ├─ 主线程：过滤图片格式，限制 ≤8 张
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

## 加载模式

拖拽文件始终以 `append=true` 追加到已有图片末尾。检查 `8 - images.len() - loading_total` 剩余名额，超出的忽略。
