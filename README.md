# herdr-reader

一个 herdr 插件：在一个很矮的分屏 pane 里读书，每一行都伪装成日志。翻页是整页替换，pane 有几行就一次换几行。

```
17:05:25 INFO  [sched ] 第1段：他推开门，屋里一片漆黑，什么也看不见……
17:05:26 INFO  [db    ] 第2段：……
```

支持 TXT（UTF-8 / GBK / GB18030）、EPUB、PDF（需要有文字层）。每本书的阅读进度会自动保存。

切换 tab / workspace 时，阅读 pane 会自动跟过去，挪到当前 pane 的下方，阅读不中断。离开一个 tab 时会在原位置留一个同样高度的占位 pane（几行静止的假日志），下次切回来直接顶替它，上面的 pane（比如 Claude Code）高度不变，不会闪、也不会画错；只有第一次进入某个 tab 时需要分屏。关闭阅读 pane 时占位 pane 会一起清掉。

## 安装

需要 herdr 0.9.3 以上版本，以及 Rust 工具链（`cargo`）：安装时会从源码编译。仅支持 Linux / macOS。

```bash
herdr plugin install 0x9bb1/herdr-reader
```

本地开发时：

```bash
bash herdr/install.sh          # cargo build --release，然后拷贝到 bin/
herdr plugin link .            # link 不会执行 build，所以要先跑上面那一步
```

## 热键（全局，焦点不用离开 agent pane）

在 `~/.config/herdr/config.toml` 里加：

```toml
[[keys.command]]
key = "f7"
type = "plugin_action"
command = "luna7.reader.toggle"   # 打开 / 关闭 pane

[[keys.command]]
key = "alt+j"
type = "plugin_action"
command = "luna7.reader.next"     # 下一页

[[keys.command]]
key = "alt+k"
type = "plugin_action"
command = "luna7.reader.prev"     # 上一页

[[keys.command]]
key = "f9"
type = "plugin_action"
command = "luna7.reader.boss"     # 老板键：切成假日志 / 切回来
```

然后执行 `herdr server reload-config`。另外还有 `open`、`close` 两个 action 可以绑定。

`alt+…` 这类组合键是直接生效的，不用先按前缀；herdr 会截获它，所以当前 pane 收不到这个键。

## pane 内按键

| 按键 | 作用 |
| --- | --- |
| `j` / 空格 / 回车 / ↓ / PgDn | 下一页 |
| `k` / ↑ / PgUp | 上一页 |
| `J` / `K` | 前进 / 后退 10 页 |
| `i` | 显示进度（伪装成 `processed 123/4567 records`） |
| `o` | 打开书库，从 `library` 目录里选书 |
| `b` | 老板键 |
| `q` | 退出（会保存进度） |

老板模式下除了 `b` 和 `q` 之外的按键都会被吞掉，误按也不会露出书的内容。

## 配置

`~/.config/herdr/plugins/config/luna7.reader/config.toml`：

```toml
library = "~/books"                                          # 按 o 时扫描的目录
modules = ["worker", "sync", "http", "db", "sched", "cache"] # 伪装用的模块名
pane_rows = 1                                                # pane 的可用行数（见下面的高度限制）
```

阅读进度保存在 `~/.local/state/herdr/plugins/luna7.reader/progress.json`。

## 高度限制

herdr 把分屏比例限制在 0.9 以内，所以阅读 pane 最矮也要占所在分屏高度的约 10%，其中边框固定占 2 行。
终端是 38 行时，pane 最矮 4 行，能显示 2 行内容；要做到只显示 1 行，终端得矮到 35 行左右以内。

## PDF

如果装了 `pdftotext`（`sudo apt install poppler-utils`）会优先用它，中文效果最好；没装就用内置的 Rust 解析器。
可以先用 `bin/herdr-reader --dump 书.pdf` 看看文字抽得怎么样。扫描版 PDF 没有文字层，读不了。
