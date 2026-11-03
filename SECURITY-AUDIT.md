# OpenIDEA 安全审计与打包安装报告

| 项目     | 内容                                                                              |
| -------- | --------------------------------------------------------------------------------- |
| 审计对象 | OpenIDEA v0.1.33（Tauri 2 + React 19 + Rust 工作区，35 个 crate）                 |
| 代码基线 | `7375f304`（`main`，工作区干净）                                                  |
| 审计时间 | 2026-11-02                                                                        |
| 审计方式 | 静态代码审查 + 配置审查 + 构建产物核验；未做动态渗透                              |
| 结论     | **未发现可直接远程利用的严重（Critical）漏洞**；有 2 项中等风险、4 项低风险需处理 |

---

## 一、总体结论

这个项目的安全架构明显是**认真设计过**的，不是"默认配置随便糊上去"的类型：

- IPC 采用**默认拒绝**模型：除只读白名单外，其余命令必须先通过项目信任检查；
- 远程开发服务端只跑 **stdio over SSH**，不开监听端口；
- 内置 Web 预览服务绑定 `127.0.0.1`、URL 带**随机 token**、Host 头校验、有目录穿越回归测试；
- Markdown 渲染统一走 **DOMPurify**，Mermaid 用 `securityLevel: "strict"`；
- 压缩包解压有 **Zip Slip / 符号链接逃逸**防护；
- 仓库中**没有硬编码密钥**，GitHub 认证复用 `gh` CLI，不自己存 token。

同时也存在一些"能再收紧一层"的地方，主要集中在 **CSP 加固**、**生产包信息暴露**、**信任模型边界**三块。以下按风险排序。

---

## 二、风险清单

| 编号 | 等级    | 标题                                                                                                    | 位置                                   |
| ---- | ------- | ------------------------------------------------------------------------------------------------------- | -------------------------------------- |
| M-1  | 🟠 中   | CSP 缺少 `object-src` / `base-uri` / `form-action` / `frame-ancestors`，且 `frame-src` 放开任意本机端口 | `src-tauri/tauri.conf.json:29`         |
| M-2  | 🟠 中   | "受限模式"边界偏窄：`fs_*`、`term_*`、`ssh_*` 前缀在不受信项目中仍全部放行（含删除文件、起 shell）      | `crates/ide-host/src/trust.rs:152-171` |
| M-3  | 🟠 中   | 首次运行把"最近项目"**自动**标记为已信任，未向用户确认                                                  | `crates/ide-host/src/trust.rs:26-33`   |
| L-1  | 🟡 低   | 生产构建开启 sourcemap，317 个 `.map` 进入安装包                                                        | `vite.config.ts:32`                    |
| L-2  | 🟡 低   | `plugin:*` 命令绕过信任检查                                                                             | `crates/ide-host/src/trust.rs:316`     |
| L-3  | 🟡 低   | `openExternalUrl` 未做协议白名单，仅靠调用方过滤                                                        | `src/platform/openUrl.ts:4-11`         |
| L-4  | 🟡 低   | Entitlements 同时放开 JIT 与"无签名可执行内存"                                                          | `src-tauri/Entitlements.plist:6-9`     |
| L-5  | 🟡 低   | `e2e-bridge` capability 随发布包一同产出（插件本身仅 debug 注册）                                       | `src-tauri/capabilities/e2e.json:6`    |
| I-1  | ⚪ 提示 | IPC 命令面 453 个，权限校验集中在单点白名单                                                             | `src-tauri/src/commands/`              |
| I-2  | ⚪ 提示 | 分发产物为 ad-hoc 签名、未公证，Gatekeeper 默认拒绝                                                     | 构建产物                               |

---

## 三、详细发现

### M-1 CSP 加固不足

**证据**（`src-tauri/tauri.conf.json:29`）：

```
default-src 'self'; img-src 'self' asset: data: blob: https://avatars.githubusercontent.com;
style-src 'self' 'unsafe-inline'; font-src 'self' data:; connect-src ipc: http://ipc.localhost;
script-src 'self' 'wasm-unsafe-eval'; worker-src 'self' blob:; frame-src http://127.0.0.1:*
```

**问题**

1. 没有 `object-src 'none'`：插件/对象类内容（`<embed>`、`<object>`）在部分 WebKit 版本仍可加载，是传统 XSS 绕过点。
2. 没有 `base-uri 'none'`：一旦存在 HTML 注入，攻击者可用 `<base href>` 改写所有相对 URL，把脚本/表单指向外部。
3. 没有 `form-action 'none'`：注入的 `<form>` 可以把工作区内容外发。
4. 没有 `frame-ancestors 'none'`：主窗口理论上可被嵌套（对本机 Tauri 应用影响有限）。
5. `frame-src http://127.0.0.1:*` 放开了**任意**本机端口。本机任何进程（含被攻破的其它应用）都可以在该端口起页面并被 IDE 预览框加载。目前风险有限（页面跨源、无法读父窗口 DOM、全仓未发现 `window.addEventListener("message")` 监听），但一旦将来加入 postMessage 通信就会出现"本机进程 → IDE 渲染层"的通道。

**修复**

```
default-src 'self';
object-src 'none';
base-uri 'none';
form-action 'none';
frame-ancestors 'none';
img-src 'self' asset: data: blob: https://avatars.githubusercontent.com;
style-src 'self' 'unsafe-inline';
font-src 'self' data:;
connect-src ipc: http://ipc.localhost;
script-src 'self' 'wasm-unsafe-eval';
worker-src 'self' blob:;
frame-src http://127.0.0.1:63342
```

即：把 `frame-src` 收敛到预览服务实际使用的端口（`PREFERRED_PORT = 63342`，见 `crates/ide-web/src/server.rs:12`），并补齐四个指令。若将来必须支持随机端口，改为在启动预览服务时动态重写 CSP，而不是留通配。

---

### M-2 "受限模式"仍放行文件与终端命令

**证据**（`crates/ide-host/src/trust.rs:152-171`）：

```rust
if ["fs_", "doc_", "syntax_", "search_", "nav_", "history_", "spell_",
     "analysis_", "structural_", "refactor_", "bytes_", "window_",
     "window_group_", "term_", "settings_", "project_trust_", "ssh_",
     "git_credential"].iter().any(|p| cmd.starts_with(p))
```

而实际命令包含：`fs_delete`、`fs_rename`、`fs_copy`、`fs_create_file`、`fs_set_writable`、`term_spawn`、`term_write`、`term_kill`、`ssh_*`。

**影响**

信任模型拦住了"打开不受信项目时**自动**执行"的路径（运行配置、LSP/DAP 启动、插件、Agent、任务），这是它的主要目的，做得对。但**受限模式并不限制文件写入和进程创建**：只要渲染层被攻破（例如 Markdown/HTML 渲染链、某个第三方依赖被投毒、未来新增的 `postMessage` 通道被利用），攻击者可以：

1. `fs_delete` / `fs_create_file` 覆写 `~/.zshrc`、`~/.ssh/authorized_keys` 等；
2. `term_spawn` 直接拿到交互式 shell → 任意命令执行。

也就是说，**信任机制不是渲染层被攻破后的第二道防线**。

**修复（按投入排序）**

1. 把 `fs_` 拆成只读前缀（`fs_read_`、`fs_probe`、`fs_entry_kind`、`fs_read_dir`…）与写前缀（`fs_delete`、`fs_rename`、`fs_copy`、`fs_create_*`、`fs_set_writable`），**写操作一律要求项目已信任**；
2. 对 `fs_*` 写入类命令在 Rust 侧校验目标路径必须位于已打开项目根内（`ide-vfs` 里已有 canonicalize 工具可复用），拒绝 `..`/符号链接逃逸；
3. `term_spawn` 保持放行（终端属于用户显式动作，与 IDEA 行为一致），但建议首次在不受信项目中打开终端时弹一次确认。

---

### M-3 首次运行自动信任历史项目

**证据**（`crates/ide-host/src/trust.rs:26-33`）：`project-trust.json` 不存在时，直接把 `RecentProjects` 里**所有**路径写入 `projects`（= 已信任）。

**影响**：迁移/首启场景下，用户历史上打开过的任意目录都会在没有二次确认的情况下获得完整 IPC 权限。如果其中某个目录后来被替换成恶意仓库（或被同步盘覆盖），用户再次打开时不会看到信任提示。

**修复**：把迁移结果写成"未决定"（新增 `pending` 列表）而不是"已信任"；下次打开时按正常流程询问一次。成本极低，收益明确。

---

### L-1 生产包携带 sourcemap

**证据**：`vite.config.ts:32` 的 `sourcemap: true` 对生产构建同样生效 —— `dist/assets/` 下有 **317 个 `.map`**，随 `frontendDist` 一起打进 `OpenIDEA.app/Contents/Resources`。

**影响**：任何人拿到 DMG 都能还原几乎全部前端源码（含内部 API 名称、注释、未使用分支），降低逆向与漏洞挖掘成本。对开源项目危害较小，但既然对外分发，建议按 profile 关闭。

**修复**：

```ts
build: { target: "safari16", sourcemap: process.env.TAURI_ENV_DEBUG === "true" }
```

（或改为 `sourcemap: "hidden"` 并只在 CI 归档时上传给错误监控。）

---

### L-2 `plugin:*` 绕过信任检查

**证据**（`crates/ide-host/src/trust.rs:316`）：`|| cmd.starts_with("plugin:")` 在受限白名单里。

**影响**：在不受信项目中仍可调用插件命令。插件本身是用户显式安装的，风险可控；但如果将来支持"项目推荐插件"或插件自动更新，这条捷径会变成绕过信任的入口。

**修复**：去掉该前缀，改为只放行插件宿主自身的只读查询（如 `plugin_list`），执行类命令走信任检查。

---

### L-3 外部链接缺少协议白名单

**证据**（`src/platform/openUrl.ts:4-11`）：

```ts
export async function openExternalUrl(url: string): Promise<void> {
  ...
  await openUrl(url);   // 任意 scheme 交给系统处理器
}
```

**现状**：Markdown 两处调用点都先做了 `/^https?:/i` 判断（`src/features/agents/Markdown.tsx:135`、`src/features/viewers/markdown/...`），所以当前**不可直接利用**。但 `openExternalUrl` 是公共工具函数，任何新增调用点忘记过滤，就会把 `file:`、`smb:`、`x-apple-*` 等 scheme 直接交给 macOS LaunchServices，可能触发本机应用行为（例如 `file:///` 打开本地文件、自定义 scheme 唤起其它应用）。

**修复**：在函数内部做白名单，把安全性从"调用方约定"变成"函数保证"：

```ts
if (!/^(https?|mailto):/i.test(url)) return;
```

---

### L-4 Entitlements 放宽 JIT 与可执行内存

**证据**（`src-tauri/Entitlements.plist:6-9`）：`allow-jit` + `allow-unsigned-executable-memory`，注释说明是给 wasmtime 插件宿主用。

**评估**：当前构建**没有启用 hardened runtime**（ad-hoc 签名，`codesign -dv` 显示 `flags=0x2(adhoc)`），这两个 entitlement 实际不生效，属于"提前量"。风险在于：一旦正式启用 hardened runtime + 公证，这两个开关会显著扩大内存被改写后的利用面。

**建议**：确认 wasmtime 是否真的需要 `allow-unsigned-executable-memory`（多数场景 `allow-jit` 即可）；若必须保留，在发布说明里标注原因，并考虑给插件宿主单独拆一个 helper 进程承载 JIT，主进程保持收紧。

---

### L-5 `e2e-bridge` capability 随包发布

**证据**：`src-tauri/capabilities/e2e.json:6` 声明 `mcp-bridge:default`；插件本身只在 debug 构建注册（`src-tauri/src/lib.rs:96`，`#[cfg(debug_assertions)]`），所以**发布版运行时不暴露该桥**，当前无可利用性。

**建议**：用 `build.rs` 或 tauri 的 `--config` 在 release profile 下移除该 capability 文件，避免将来有人把 `#[cfg(debug_assertions)]` 去掉时"顺手"把自动化桥带进正式版（该桥绑定 `127.0.0.1`，可被本机任意进程驱动 UI 与命令）。

---

### I-1 IPC 命令面 453 个

`src-tauri/src/commands/` 下共约 453 个 `#[tauri::command]`。权限校验集中在 `ide-host::trust::authorize_command` 单点，属于"单点失效即全面失守"的结构。

**建议**：为高风险命令（`*_spawn`、`*_exec`、`*_write`、`*_delete`、`http_*`、`remote_*`）加第二道参数级校验；对 `remote_invoke` 保持现有的逐命令授权（`src-tauri/src/remote_dev.rs` 已实现，做法正确）。

---

### I-2 分发签名为 ad-hoc、未公证

`codesign --verify` 通过、`spctl` 拒绝。本地构建、本地安装不受影响（无 quarantine 属性），但**通过 DMG 分发给他人**时会被 Gatekeeper 拦下。

正式发布路径：Developer ID Application 证书 + `notarytool` 公证 + stapler 装订；CI 中已预留 `APPLE_*` 变量（`.github/workflows/release.yml:105-120`），填上 secrets 即可。

---

## 四、已核查且实现良好（可作为正面结论）

| 项                            | 证据                                                                                                                            | 评价                               |
| ----------------------------- | ------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------- |
| 自动化桥仅 debug              | `src-tauri/src/lib.rs:96` `#[cfg(debug_assertions)]`                                                                            | 正确，发布版无 MCP 桥              |
| 远程开发服务端无监听端口      | `crates/ide-host/src/bin/server.rs` 仅 `--stdio`，日志走 stderr                                                                 | 信任边界交给 SSH，设计正确         |
| 预览服务绑定回环 + 随机 token | `crates/ide-web/src/server.rs:46,104`；`:334-340` 有穿越与 Host 校验测试                                                        | 防本机其它进程探测，很好           |
| Markdown 统一消毒             | `src/features/viewers/markdown/render.ts:84` DOMPurify + `ADD_ATTR` 白名单；图片改 `data-src` 延迟加载                          | 正确                               |
| Mermaid 严格模式              | `src/features/viewers/markdown/mermaid.ts:93` `securityLevel: "strict"`                                                         | 正确                               |
| 解压防护                      | `crates/ide-tools/src/archive.rs:107-145`：`enclosed_name` + `is_safe_relative` + `reject_symlink_components` + `validate_link` | 防 Zip Slip / 链接逃逸，做得完整   |
| 无硬编码密钥                  | 全仓正则扫描 `sk-*`/`ghp_*`/`AKIA*`/`PRIVATE KEY` 无命中                                                                        | 通过                               |
| GitHub 认证                   | `crates/ide-github/src/lib.rs:2` 复用 `gh auth`，不自存 token                                                                   | 通过                               |
| 默认拒绝的 IPC 模型           | `crates/ide-host/src/trust.rs:320-326`                                                                                          | 方向正确（边界见 M-2）             |
| 工作区信任不来自项目配置      | `crates/ide-host/src/trust.rs:1` 注释与实现一致                                                                                 | 防"仓库自带配置自我授权"，关键设计 |

---

## 五、加固优先级

1. **M-3**（自动信任历史项目）——改动 5 行，收益最高；
2. **M-2**（`fs_*` 写操作纳入信任检查 + 路径作用域校验）——改动中等，堵住"渲染层失守 → 任意文件/进程"的链路；
3. **M-1**（补 CSP 指令、收敛 `frame-src` 端口）；
4. **L-1**（生产关闭 sourcemap）；
5. **L-2 / L-3 / L-5**（去 `plugin:*` 捷径、URL 白名单、release 剔除 e2e capability）；
6. **L-4 / I-2**（正式发布前处理签名、公证与 entitlement 收敛）。

---

## 六、打包与安装结果

### 构建环境

| 项       | 值                                                                                                     |
| -------- | ------------------------------------------------------------------------------------------------------ |
| 机器     | Apple Silicon（arm64），macOS 15.1，10 核 / 16 GB                                                      |
| Node     | v24.12.0（Volta）                                                                                      |
| pnpm     | 10.34.5                                                                                                |
| Rust     | **1.99.1 stable**（仓库 `rust-version = "1.96"`，原有 1.91-nightly 不满足；已通过 rustup 安装 stable） |
| 构建耗时 | 30 分 18 秒（release，LTO + `codegen-units = 1`）                                                      |

> 注：`pnpm tauri build` 在本机会失败（`scripts/tauri.ts` 被 pnpm 用 Node 20 执行，触发 `ERR_UNKNOWN_FILE_EXTENSION`），因此改用 `node node_modules/@tauri-apps/cli/tauri.js build` 直接调用，并用 `RUSTUP_TOOLCHAIN=stable` 指定工具链。这是**环境问题**，不是仓库缺陷。

### 产物

| 产物 | 路径                                                    | 大小       |
| ---- | ------------------------------------------------------- | ---------- |
| App  | `target/release/bundle/macos/OpenIDEA.app`              | 138.94 MiB |
| DMG  | `target/release/bundle/dmg/OpenIDEA_0.1.33_aarch64.dmg` | 49 MB      |

```
SHA-256(OpenIDEA_0.1.33_aarch64.dmg) =
98bdd15c1cb7945b6d7628a5e9753b104d64775d0722e8219dcbeaca0b32c3fe
```

DMG 内容：`OpenIDEA.app` + `/Applications` 软链接（拖拽安装布局）。

### 签名

Tauri CLI 未对产物做 bundle 级签名，原始 `.app` 只有链接器 ad-hoc 签名，`codesign --verify` 报 `code has no resources but signature indicates they must be present`。已重新以 ad-hoc 方式签名（保留 Entitlements）：

```
codesign --force --sign - --entitlements src-tauri/Entitlements.plist OpenIDEA.app
```

之后：

- `codesign --verify --strict --deep` → `valid on disk` / `satisfies its Designated Requirement` ✅
- `spctl -a` → `rejected`（未公证，符合预期）⚠️

### 安装

| 项       | 值                                                                                             |
| -------- | ---------------------------------------------------------------------------------------------- |
| 安装位置 | `/Applications/OpenIDEA.app`（139 MB，版本 0.1.33）                                            |
| 安装方式 | 挂载 DMG → `ditto` 复制 → 卸载 → 清除 quarantine                                               |
| 运行状态 | ✅ 已启动，进程 `87573` `/Applications/OpenIDEA.app/Contents/MacOS/openidea`                   |
| 启动日志 | `~/Library/Logs/ai.l0ng.openidea/openidea.2026-11-02.log` 显示项目加载、索引、LSP 初始化均正常 |

> 若以后从网上下载同款 DMG，首次打开会被 Gatekeeper 拦截，可在"系统设置 → 隐私与安全性"里点"仍要打开"，或执行：
> `xattr -dr com.apple.quarantine /Applications/OpenIDEA.app`

---

## 七、附：环境提醒（与仓库无关）

审计过程中发现 `~/.npmrc` 中存有**明文 npm registry token**（含内网私服地址）。这不是仓库问题，但建议：

1. 轮换该 token；
2. 改用 `npm login`（token 存 macOS Keychain）或 `keytar` 类方案；
3. 不要把 `~/.npmrc` 粘贴到聊天、工单或日志里。

---

_本报告仅覆盖上述基线版本的静态审查；动态模糊测试、依赖供应链审计（`cargo deny` / `pnpm audit`）与插件沙箱逃逸测试未包含在内，建议后续补做。_
