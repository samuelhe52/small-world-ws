# Small World Lab

Small World Lab 是一个基于浏览器的演示项目，用于运行真正分区存储的
Watts-Strogatz 实验。Rust 协调器会启动多个独立的 worker 进程。每个 worker
只存储分配给自己的节点范围及其邻接表；协调器只保存进度元数据，不保存图的
邻接数据。

仪表盘支持调整 `N`、`K`、重连概率 `p`、BFS 样本数量和 worker 进程数量。它会
实时显示图构建、重连、聚类和分布式 BFS 的进度，随后展示 `L`、`C`、运行详情及
历史记录图表。

## 启动仪表盘

```bash
./scripts/run_web.sh
```

然后打开 [http://127.0.0.1:8080](http://127.0.0.1:8080)。

该脚本会安装并构建 React 前端，然后启动 release 模式的 Rust 服务器。也可以
手动执行这两个步骤：

```bash
cd frontend
npm install
npm run build
cd ..
cargo run --release -- serve
```

默认工作负载为 `N=1,000,000`、`K=10`、`p=0.05`、32 个分布式 BFS 源点和 4 个
worker 进程。UI 允许最多 500 万个节点和 8 个 worker。

## 哪些部分是真正分布式的？

- 每个 worker 都是拥有独立地址空间的单独操作系统进程。
- worker `i` 负责一个连续的节点 ID 范围，并且只存储该范围内节点的邻接表。
- 协调器和浏览器服务器都不会保存完整的图副本。
- 重连操作会将远端端点的添加或删除请求发送给对应的所属进程。
- 聚类计算会将邻居边是否存在的查询发送给存储目标邻接表的进程。
- 每次采样 BFS 都按层同步：worker 扩展本地 frontier，返回远端发现结果，接收
  路由后的发现结果，然后共同进入下一层。
- 消息通过子进程管道中的定长前缀 `bincode` 协议传输。

`C` 最多从 20,000 个均匀选取的顶点中采样，因此百万节点规模的运行仍能保持
交互性。`L` 使用可配置数量的均匀选取源点进行采样。两个样本都不放回，并且
对于选定的 seed 是确定性的。

协议和正确性细节请参阅 [ARCHITECTURE.zh-CN.md](ARCHITECTURE.zh-CN.md)。

## 验证

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cd frontend && npm run build
```

早期基于共享内存的 Rayon CLI 仍可作为对比基线使用：

```bash
cargo run --release -- demo
cargo run --release -- accuracy --nodes 5000 --samples 100
```
