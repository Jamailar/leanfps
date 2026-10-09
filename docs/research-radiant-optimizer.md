# 游戏性能优化软件调研：Radiant Optimizer（2026-10-09）

用户的想法：radiantoptimizer.com 的产品做得一般，优化游戏性能是系统工程，我们可以用 Rust 做一个更好、安装包更小的。本文只做调研，还没开工。

调研方式：`curl` 读了官网首页、对比页、退款政策、隐私政策、评价页、新闻页、`robots.txt`、`sitemap.xml`；下载官网的免费安装包 `RadiantOptimizer_Setup_v3.3.8.exe`（SHA-256 `4ae3da96…5f37035`），**只解包和读字符串，没有运行**。竞品只读了 hone.gg 首页和 tier1settings.com 的一篇对比文章，其余来自搜索摘要（已注明）。

## 1. 结论先说

1. **它已经是 Rust 做的**：主程序是 Tauri 2.11.1（Rust + WebView2），安装器是 Tauri 生成的 NSIS。所以「用 Rust 做」本身不是差异点，差异要落在**体积、可测量、可撤销、诚实**上。
2. **安装包 121 MB，几乎全是可以省掉的东西**（见第 3 节）。我们用 Rust 原生读传感器、不打包 .NET 和壁纸，安装包有希望做到 10 MB 以内（目标值，未验证）。
3. 它最大的软肋不是技术栈，而是**没有证据**：首页的性能图全部标着「Illustration only. Not a lab benchmark」，评价只有 13 条且只能已购用户写；它没有内置「优化前后对比测试」。我们最该做的差异化是：**内置帧时间测量，用户自己的电脑上跑优化前/后，给出平均帧率和 1% low 的真实对比。**
4. 它有几处会让懂行用户不信任的地方：关闭 VBS/HVCI（内存完整性）这类降低安全性的选项、网络「重置」类操作混在优化里、退款口径前后矛盾、数字不一致。

## 2. 官网与商业模式（读取日期 2026-10-09）

| 项目 | 内容（来源） |
| --- | --- |
| 定位 | 「PC Optimizer for Competitive Gamers」，Windows 10/11；口号 Stop losing fights to your PC（首页） |
| 宣称规模 | 400+ 优化项、16 个硬件诊断、20 个游戏配置、5 个电源计划（首页）；对比页却写 22 个诊断工具、18 个游戏预设、另一处写 15 个硬件测试 —— **数字前后不一致** |
| 定价 | 免费版（可浏览全部页面、应用部分优化，无需账号）+ 终身版 **$57 一次性**；主打「对比月付 $8–$32 的竞品」（首页、对比页） |
| 退款 | 退款政策：「completed purchases are final」，只有重复扣款等例外，拒付会吊销许可；**但对比页写「30-day money-back guarantee」**，两处矛盾 |
| 许可 | 绑定主板派生的硬件 ID，换主板需进 Discord 申请转移（FAQ、退款政策） |
| 支持 | 只有 Discord 工单，没有邮箱（退款政策第 7 节） |
| 信任要素 | 「2× WSOW LAN Finalist」作者背景、反作弊安全声明（Vanguard、Ricochet、EAC、FACEIT）、还原点、评价页（13 条、平均 5.0、仅已购用户可写，内容由 JS 从 Firestore 加载） |
| 页面 | 首页、Warzone 专题页、评价、对比页、新闻（JS 加载，静态 HTML 里是空的）、合作伙伴、退款/隐私/条款、注册登录 |
| 技术 | 静态页 + Firebase（Auth、Firestore、App Check）+ Stripe；后端 `radiant-backend-xoxk.onrender.com`、`ro-enrich.frosty-night-93c3.workers.dev`；访客 IP 发给 ipapi.co 做地理定位（隐私政策） |
| 小问题 | `robots.txt` 里直接写出了管理页路径 `/ro-control-7f3k9.html`；`sitemap.xml` 注释里留着开发说明 |

值得学的：免费版不限时、不要卡，先试再买；对「hit reg」的说法很克制（明说服务器权威、只能改善本机包处理）；FAQ 直接回答反作弊、撤销、是否常驻、换主板。
不该学的：全是示意图没有实测；数字和退款口径不一致；支持只靠 Discord。

## 3. 安装包拆解（v3.3.8，121 MB）

| 文件 | 大小 | 是什么 |
| --- | --- | --- |
| `radiant-optimizer-v2.exe` | 60 MB | Tauri 主程序（Rust），前端资源打包在内：19 个游戏 logo（jpg/png/webp）等 |
| `sensor-host/sensor-host.exe` | 37 MB | .NET 自包含单文件（含 CoreCLR），用来读硬件传感器 |
| `wallpapers/` | 27 MB | 10 张 1080p/1440p 的 PNG 品牌壁纸 |
| `lhm/` | 12 MB | LibreHardwareMonitor 完整 GUI 版及依赖，**带 `.pdb` 调试符号**；库里使用 PawnIO 驱动 |
| `LibreHardwareMonitor/` | 5.5 MB | **又一份** LibreHardwareMonitor.exe + Lib |

安装包里没看到 LibreHardwareMonitor（MPL-2.0）的许可证文件（只是没找到，未做法律判断）。

体积来源总结：两套 .NET 传感器方案（共约 55 MB）+ PNG 壁纸 27 MB + 主程序里的图片。真正的优化逻辑很小。

## 4. 它实际改了什么（从主程序字符串看到的，未运行验证）

优化主要是**主程序里内嵌的 PowerShell 脚本**，另有 `fixId`、`planName`、`downloadUrl` 等字段，可能还有服务端下发的配置（未确认）。看到的操作：

- 电源：`powercfg -duplicatescheme e9a42b02-…`（复制「卓越性能」计划）、`SCHEME_MIN`、关闭快速启动（`HiberbootEnabled=0`）。
- 启动参数：`bcdedit`（具体参数没在字符串里读到）。
- 安全相关：检测并切换 **HVCI / Credential Guard**（`$hvci = 'Off'; $cg = 'Off'`）；Defender 快速扫描、重启 Defender 服务。
- 系统服务：停止/改启动类型 `wuauserv`、`BITS`、`cryptsvc`、`msiserver`（看起来是「修复 Windows 更新」流程）。
- 网络「修复」：`netsh winsock reset`、`int ip reset`、`advfirewall reset`（**会把防火墙恢复成默认规则**）、清 ARP 缓存、重置 WinHTTP 代理。
- 其他：Appx 预装应用卸载、显卡调度 `HwSchMode` 读取、着色器缓存清理、计时器精度（`NtSetTimerResolution`，FAQ 说是唯一需要常驻的功能）、导出 `HKCU/HKLM` 的 `.reg` 备份、创建系统还原点、Cloudflare 测速（`speed.cloudflare.com/__down`、`__up`）。
- 许可校验：Firebase（identitytoolkit、Firestore），许可存在 `HKCU\SOFTWARE\RadiantTech\RadiantOptimizerV1`。

判断：这些是社区里常见的那一套调参（与 WinUtil、各种 tweak 包重叠很大），没有看到针对具体硬件的测量闭环。关闭 HVCI、重置防火墙这类操作对普通玩家有安全代价，应当默认不做，或在界面里写清楚后果。

## 5. 竞品与市场

| 竞品 | 读到的内容 | 来源 |
| --- | --- | --- |
| Hone.gg | 免费 10 项优化，推荐好友每人 +1 最多 15；Premium 解锁全部、预设、去广告；注册表优化前备份原值可恢复；网页安装器经 **Overwolf** 分发，也上架 Epic | 读了 hone.gg 首页 |
| Hone 用户反馈 | Trustpilot 上有人说帧数提升，也有人说掉 5–10 帧、驱动问题、无法开机 | 搜索摘要（未读原文） |
| WinUtil（Chris Titus） | 免费 PowerShell 工具，200+ 贡献者，最强的免费精简工具 | 读了 tier1settings 文章 |
| Process Lasso | 适合系统繁忙时的卡顿（CPU 调度） | 同上 |
| Razer Cortex | 启动游戏时清后台，作者认为在内存足够的现代 PC 上效果很小 | 同上 |
| Tier1Tune | 33 项、每项写明改了什么、实测影响、风险等级、一键撤销；免费 20 项 | 同上（这篇文章本身是 Tier1 自家产品的软文） |
| 「PC 清理/注册表优化」类 | 该文认为大多是安慰剂 | 同上 |

市场共识（多个搜索摘要一致，未读原文）：玩家和媒体对「FPS booster」普遍怀疑，认为后台类调参收益小，真正大的收益来自游戏内设置和驱动。**这正是我们的机会：谁能在用户自己的电脑上证明有效，谁就赢得信任。**

## 6. 用户痛点 → 我们的设计

| 痛点 | 设计 |
| --- | --- |
| 不知道到底有没有用（全是示意图） | 内置帧时间测量（基于 ETW/PresentMon 思路）：选游戏 → 跑 60 秒基线 → 应用 → 再跑 → 输出平均帧、1% low、帧时间方差的前后对比卡片，可分享 |
| 怕改坏、不知道改了什么 | 每一项写明：改哪个注册表/服务/命令、预期影响、风险等级、来源；每项单独撤销；应用前自动备份原值 + 还原点 |
| 安全顾虑（关 HVCI、Defender、防火墙） | 降低安全性的项默认关闭并单独归类为「有安全代价」，需二次确认；永不禁用 Defender |
| 反作弊封号担心 | 只改系统配置、不注入游戏、不常驻；计时器精度用系统 API 并说明 |
| 安装包大、后台常驻 | 目标安装包 < 10 MB、无 .NET 依赖、应用后可关闭 |
| 月付/许可麻烦 | 可沿用「免费 + 一次买断」，许可不绑主板（或宽松绑定 + 自助转移） |
| 支持只有 Discord | 邮箱 + 网站帮助中心（走共享网关和 `contact@` 收件箱） |

## 7. 技术方向建议（待讨论）

- 语言：Rust。界面二选一：
  - **Tauri 2 + 轻量前端**：开发快，但依赖系统 WebView2；Radiant 就是这个，体积大是它自己堆资源造成的，不是 Tauri 的锅。
  - **原生 Rust UI（Slint 或 egui）**：单个 exe，体积最小、启动最快，最能兑现「更小」的承诺。
- 传感器：Rust 直接调用 WMI / NVAPI / ADLX / PDH，不打包 LibreHardwareMonitor；需要内核驱动的读数（如 CPU 温度的 MSR）先不做或按需下载。
- 优化项做成**数据驱动的清单**（TOML/JSON：检测条件、应用、撤销、风险、证据链接），和程序分开发布、可签名更新。
- 代码签名证书（EV 或 Azure Trusted Signing）是必需项，否则 SmartScreen 和杀软会拦。
- 官网走本仓库的 Next.js 模板，账号/付款/许可走共享网关（需要确认网关能否发放和校验桌面软件许可，可能要给网关提 PR）。

## 8. 调研限制

- 安装包没有运行，第 4 节来自字符串，不代表全部行为；前端资源被压缩，界面里的完整优化清单没读到；是否有服务端下发配置未确认。
- 评价页、新闻页由 JS 加载，没用浏览器渲染，只看到「13 条、5.0」。
- Reddit 等论坛没有搜到关于 Radiant Optimizer 的讨论（搜索结果为空），说明它体量还小；Hone 的用户反馈只看到搜索摘要。
- 没读 Warzone 专题页、条款、合作伙伴页的全文。

## 9. 需要用户决定的事

1. 产品名和域名（还没定）。
2. 商业模式：一次买断（对标 $57）还是免费 + 订阅？我的建议是免费版就能测量和应用核心项，买断解锁全部 + 游戏配置。
3. 界面路线：Tauri 还是原生 Rust UI（我倾向原生，最能体现「小」）。
4. 是否购买代码签名证书（约每年数百美元，没有它下载转化会很差）。
