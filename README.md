# Slint Rust GUI 应用

这是一个使用 Slint 和 Rust 开发的简单 GUI 应用程序。当前使用 Slint 1.12.1 版本。

## 项目结构

```
slint_app/
├── Cargo.toml          # Rust 项目配置文件
├── build.rs           # 构建脚本，用于编译 Slint UI 文件
├── src/
│   └── main.rs        # Rust 主程序代码
└── ui/
    └── main.slint     # Slint UI 界面定义
```

## 功能介绍

这个应用程序提供了一个简单的 GUI 界面，包含以下功能：

- 文本显示区域
- 文本输入框
- 交互按钮

用户可以在输入框中输入文本，点击按钮后，应用会显示用户输入的内容。

## 运行方法

确保你已安装 Rust 和 Cargo：

```bash
# 检查 Rust 版本
rustc --version

# 检查 Cargo 版本
cargo --version
```

然后，按照以下步骤运行应用：

1. 克隆或下载项目代码
2. 进入项目目录：`cd slint_app`
3. 编译并运行项目：`cargo run`

## 技术栈

- [Rust](https://www.rust-lang.org/) - 系统编程语言
- [Slint](https://slint.dev/) - 跨平台 UI 开发框架
  - [Slint crates.io](https://crates.io/crates/slint) - Rust 包
  - [Slint 文档](https://docs.rs/slint/1.12.1/slint/) - API 文档

## 扩展开发

如果你想扩展这个应用，可以：

1. 修改 `ui/main.slint` 文件来更改 UI 设计
2. 在 `src/main.rs` 中添加更多的业务逻辑
3. 添加更多的 Slint 组件和交互功能

## Slint 现代 API 注意事项

### 版本兼容性

Slint 遵循语义化版本控制原则，1.x 系列保持稳定的 API。本项目使用 Slint 1.12.1 版本，包含以下现代 API 特性：

### UI 运行方式

在 Slint 1.x 中，启动 UI 的推荐方式是：

```rust
main_window.show()?;
slint::run_event_loop();
```

而不是旧版本的 `main_window.run()`。

### 组件和布局

- 使用 `VerticalBox` 和 `HorizontalBox` 进行布局
- 使用 `Palette` 命名空间访问主题颜色（如 `Palette.window-background`）
- 使用 `StyleMetrics` 获取系统样式信息

### 资源管理

在 `build.rs` 中，可以使用 `slint_build::compile` 或 `slint_build::compile_with_config` 配置资源嵌入方式。

### 文档资源

- [Slint 官方文档](https://docs.slint.dev/)
- [Slint 语言参考](https://docs.slint.dev/slint-reference.html)
- [Slint 标准组件库](https://docs.slint.dev/slint/src/reference/widgets/std-widgets.html)
- [Slint GitHub 仓库](https://github.com/slint-ui/slint)