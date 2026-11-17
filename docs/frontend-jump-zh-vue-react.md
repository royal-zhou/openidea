# 系统语言中文 + Vue/React 跳转（含 pnpm 工作区与别名）支持方案

> 状态：已按确认范围收敛 · 代码基线：`main@7375f304` · 版本：0.1.33

## 0. 已确认的范围

| 决策          | 结论                                                                               |
| ------------- | ---------------------------------------------------------------------------------- |
| “中文”        | **系统语言（界面）支持中文**，并跟随系统语言；不是代码里的中文标识符               |
| 跳转范围      | Vue（含 Vue 2）、React 18，重点是路径别名                                          |
| 与 LSP 的关系 | **LSP 之上补 vite 别名**：LSP 仍是第一来源，内置解析器只在 LSP 无答案/不可用时兜底 |
| 配置解析边界  | 接受**静态解析 + 设置项兜底**，不执行项目里的 JS 配置                              |
| 工作区        | 必须支持 **pnpm workspace**                                                        |

两条主线可以独立上线：A（系统语言）约 2 天，B（跳转）按下面的里程碑推进。

## 1. 系统语言：现状

### 1.1 已经具备的

- 中英文案**已全量**：`en.json` / `zh.json` 各 448 个 key，key 集合完全一致；`locales/text/zh.json`
  有 5502 条源码文案，逐条与英文不同（没有“英文占位”）。
- `textCatalog.test.ts` 已经守住“每个字面量 UI 文案都有条目”和“没有过期条目”。
- 运行时切换可用：设置页 `appearance.locale`（`src/features/settings/pages/behavior.tsx:686`）
  提供 `en` / `zh`，`setLocale` 懒加载目录并触发重渲染。
- 原生菜单通过 `buildMenuConfig()` 推送本地化标签（`src/features/app/menuBridge.ts:44-54`），
  locale 变化时重新推送（同文件 142 行）。
- 设置窗口也有 locale 激活入口（`activateLocale`，`src/features/app/index.ts:24`）。

### 1.2 缺口

| 问题                                                | 位置                                                             | 影响                                              |
| --------------------------------------------------- | ---------------------------------------------------------------- | ------------------------------------------------- |
| **没有“跟随系统”选项**，默认 `en`                   | `schema.ts:37,174`、`settings.rs:362`、`behavior.tsx:686-696`    | 中文系统用户首次启动看到英文，必须手动切换        |
| `resolveLocale` 已支持 `system`，但 UI 产不出这个值 | `i18n.ts:58-64`                                                  | 能力存在但不可达                                  |
| `documentElement.lang` 两处写入且不一致             | `i18n.ts:109`（`zh`）与 `settings/runtime.ts:23`（`zh-CN`/`en`） | `lang` 属性错误；`system` 时会被写成 `en`         |
| locale 在设置异步加载后才应用                       | `installLocaleSync`（`features/app/locale.ts:11-25`）            | 首屏先渲染英文，再闪成中文                        |
| 字体栈没有中文回退                                  | `styles/tokens.css:212,215`                                      | 依赖系统兜底字体，Windows/Linux 上字重/行高不稳定 |

## 2. 系统语言：方案

### A1 设置项与默认值

- `LocaleId` 增加 `"system"`（`features/settings/model/schema.ts:37`），`LOCALES` 同步
  （同文件 174 行），`DEFAULT_SETTINGS.appearance.locale = "system"`（122 行）。
- Rust 端 `AppearanceSettings::default().locale = "system"`（`crates/ide-core/src/settings.rs:362`）。
- 设置页选项：`System (follow the OS) / 跟随系统语言`、`English`、`简体中文`；默认选中“跟随系统”。
- **迁移**：已保存 `en`/`zh` 的用户保持原选择；只有缺失或值为 `system` 时才跟随系统。

选“跟随系统”时存的是 `system` 而不是解析后的 `zh`，这样用户改系统语言后重启即可生效。

### A2 语言解析

`resolveLocale` 拆成纯函数（可注入 tag，便于测试）：

| 输入                                 | 结果                                                   | 说明                         |
| ------------------------------------ | ------------------------------------------------------ | ---------------------------- |
| `system` / `auto` / 空               | `navigator.languages[0]` → `navigator.language` → `en` | 现有逻辑保留                 |
| `zh`、`zh-CN`、`zh-Hans`、`zh-SG`    | `zh`                                                   | 简体目录                     |
| `zh-TW`、`zh-HK`、`zh-Hant`、`zh-MO` | `zh`（暂）                                             | 目前只有简体目录；见待确认 2 |
| `en`、`en-US`、`en-GB`               | `en`                                                   |                              |
| 其他（`fr`、`ja`…）                  | `en`                                                   | 未支持语言回退英文           |

### A3 启动时序：首屏就是中文

现在 locale 在 `activate()` 里等设置返回后才应用，首屏必然先英文。改为在渲染前完成：

```ts
// src/features/app/locale.ts
export async function primeLocale(): Promise<void> {
  let value: string | undefined;
  try {
    value = isTauri() ? (await commands.settingsGet()).appearance?.locale : undefined;
  } catch {
    // 设置读不出来就跟随系统
  }
  await setLocale(value ?? "system");
}
```

在 `src/main.tsx` 的 `ReactDOM.createRoot(...).render(...)` 之前 `await primeLocale()`；
`hideSplash()` 保持在渲染之后，启动画面覆盖这段等待。设置窗口走同一个 `main.tsx`，因此
`activateSettingsWindow()` 也拿到已就绪的 locale。

代价：一次 `settingsGet` IPC + 首次加载 `zh.json` / `text/zh.json` chunk（本地资源，目标 <50 ms）。
若实测超预算，就把 `zh.json`（448 个 key，体积很小）改为静态 import，只让 320 KB 的
`text/zh.json` 懒加载。

### A4 `lang` 属性单点维护

- `setLocale` 里统一写：`document.documentElement.lang = locale === "zh" ? "zh-CN" : "en"`。
- 删除 `features/settings/runtime.ts:23` 的 `root.lang = ...`（它按原始值判断，`system` 时会写错）。

### A5 覆盖核查与守卫

现状已经全量，方案是**加护栏，防止以后回退**：

- 新增测试：`zh.json` 的 key 集合与 `en.json` 完全一致；`text/zh.json` 中不允许出现
  “值等于英文 key”的条目（当前 0 条，允许 `UNTRANSLATED` 白名单，如产品名）。
- 后端字符串：`textCatalog.test.ts` 的 `scanBackend` 已覆盖 Rust 字面量；对拼接出来的消息
  做一轮人工走查（错误弹窗、安装器进度、语言服务器状态）。
- 人工验收面：原生菜单、设置窗口、Welcome、各工具窗、状态栏、对话框、通知、Keymap 页、
  About、更新提示、工具安装器。

### A6 中文排版与输入

- `styles/tokens.css` 的 `--font-ui` / `--font-mono` 追加 CJK 回退：
  `"PingFang SC", "Hiragino Sans GB", "Microsoft YaHei", "Noto Sans CJK SC", "Source Han Sans SC"`。
  编辑器字体同样加回退，避免 JetBrains Mono 缺字时行高跳动。
- 检查 13px/22px 下中文是否被裁切（`EditorView`、`Tabs`、状态栏）；中文标签普遍比英文短，
  重点看 `min-width` 与省略号。
- 输入法：`isImeKey`（`src/platform/ime.ts`）已存在，确认快捷键分发、补全确认、导航手势在
  组合输入期间都被忽略；补单元测试（`compositionstart` → 按键 → `compositionend`）。
- 数字/日期：`Intl` 使用解析后的 locale；日期格式仍按设置页的 pattern，不在本次改动。

### A7 验收

1. 中文系统首次启动：无英文闪烁，菜单/欢迎页/设置均为中文。
2. 设置里切 English → 立即变英文；切回“跟随系统” → 立即变中文。
3. 中文系统重启应用 → 仍是中文；`document.documentElement.lang === "zh-CN"`。
4. 设置窗口标题、菜单、通知同步切换。
5. 中文输入法下 ⌘B / ⌘-click / 补全不误触发。

## 3. 跳转：现状

### 3.1 链路

`gotoTargets()`（`src/editor/lsp/navigation.ts:160`）：LSP → Web 引用（class/id）→
链接/文件引用 → 语法回退。文件引用最终到 `file_refs::resolve()`
（`crates/ide-workspace/src/file_refs.rs:104`），只做“相对目录/项目根/资源根 + 路径存在”的精确匹配。

### 3.2 能力现状

| 能力                            | 现状                                                                    | 依据                                |
| ------------------------------- | ----------------------------------------------------------------------- | ----------------------------------- |
| tsconfig `paths` / `baseUrl`    | LSP 可用（vtsls/Volar 读取）                                            | 别名唯一来源                        |
| vite / vitest `resolve.alias`   | ✗                                                                       | 无解析器；tsserver 不读 vite 配置   |
| vue.config.js / webpack alias   | ✗                                                                       | 同上                                |
| pnpm workspace 包（`@acme/ui`） | ✗                                                                       | 无 workspace 概念                   |
| Vue 2 SFC（`.vue`）             | 仅 LSP；`@vue/language-server` 3.1 起移除 Vue 2 支持，当前安装 `latest` | `languages/vue.toml` 无 tree-sitter |
| React 18（TSX）                 | 语法层完整；模块解析缺别名/省略扩展名                                   | `tsx` grammar 存在                  |
| 中文路径（别名里带中文）        | 裸中文文件名识别失败                                                    | `links.ts:75` 的 ASCII `\w`         |

### 3.3 结论

- 别名只有 LSP 一条路，`vite`/`vue.config.js` 配的别名在 LSP 解析不到时直接失败。
- workspace 多包项目里，tsconfig 向上查找会越过包边界，`@acme/ui` 没有包映射。
- Vue 2 的 `.vue` 文件在 LSP 之外没有任何内置兜底；而当前 `latest` 的 Vue 语言服务器已不再支持 Vue 2。

## 4. 跳转：方案

### B1 原则

1. **LSP 优先**：服务端返回了定义就不覆盖；只在返回空、报错或服务未就绪时启用解析器。
2. **静态解析 + 设置兜底**：解析 tsconfig/vite/vue.config.js 的字面量配置；执行不了的动态配置
   由新增设置项 `languages.typescript.pathAliases` 手填。
3. **工作区感知**：从文件所在目录向上找最近的 tsconfig / vite 配置，最远到 pnpm workspace 根。
4. **结果可解释**：每个目标带来源（`tsconfig paths "@/*"`、`vite alias "@"`、`workspace package`），
   失败提示原因。

### B2 模块解析器（Rust）

新模块 `crates/ide-workspace/src/module_refs/`，复用 `file_refs` 的路径规范化：

```rust
pub struct ModuleRequest<'a> {
    pub from: &'a Path,
    pub specifier: &'a str,
}

pub struct ModuleTarget {
    pub path: PathBuf,
    pub dir: bool,
    /// 命中来源，UI 用来显示“为什么能跳”
    pub via: AliasSource,
}

pub enum AliasSource {
    Relative,
    TsconfigPaths,
    TsconfigBaseUrl,
    ViteAlias,
    VueCliAlias,
    WebpackAlias,
    WorkspacePackage,
    PackageImports,
    Framework,  // Nuxt `~`/`@`、Vue CLI `@` 默认值
    Settings,
}

/// 0 个 = 失败；1 个 = 直接跳；多个 = 弹选择器。
pub fn resolve(project: &ProjectConfig, req: &ModuleRequest) -> Vec<ModuleTarget>;
pub fn aliases(project: &ProjectConfig, from: &Path) -> Vec<AliasInfo>;
```

### B3 来源与优先级

| 优先级 | 来源                                    | 解析方式                                                                      |
| ------ | --------------------------------------- | ----------------------------------------------------------------------------- |
| 1      | LSP（本模块之外）                       | tsserver / Volar 的 `definition`                                              |
| 2      | 相对路径 `./` `../` `/`                 | 现有 `file_refs` 规则 + 扩展名探测                                            |
| 3      | tsconfig / jsconfig `paths` + `baseUrl` | JSONC、`extends` 链、最长前缀、`*` 通配                                       |
| 4      | vite / vitest `resolve.alias`           | 静态 AST：对象/数组、`path.resolve`、`new URL`、`fileURLToPath`               |
| 5      | vue.config.js / webpack `resolve.alias` | 静态 AST；`chainWebpack` 只认简单 `alias.set`                                 |
| 6      | workspace 包                            | `package.json` 的 `exports` / `module` / `main` / `types`，再退 `src/index.*` |
| 7      | 框架约定                                | Nuxt 2/3 `~`/`@`、`~~`/`@@`；Vue CLI `@` → `src`                              |
| 8      | `package.json#imports`                  | `#foo` 映射                                                                   |
| 9      | 用户设置                                | `languages.typescript.pathAliases`                                            |

同一优先级内多个命中时按“同语言族 → 非 `.d.ts` → 路径长度 → 字典序”排序，全部返回给前端做选择器。

### B4 解析算法

```text
resolve(from, spec):
  spec 为空 / URL / data: / node:                 → []
  spec = percent_decode(strip_query_hash(spec))    // ?raw、?url、#frag
  spec 以 ./ ../ / 开头                           → probe(...)          // 复用 file_refs
  cfg = config_for(from, workspace)                // 最近 tsconfig + 最近 vite
  最长前缀别名命中                                → probe(target + rest)
  baseUrl 命中                                    → probe(baseUrl / spec)
  spec 是 workspace 包名[/子路径]                 → package_entry(...)
  spec 以 # 开头                                  → package.json imports
  否则                                            → []（第三方包不在本期）

probe(p):
  文件(p) → [p]
  否则尝试 EXTS：ts tsx js jsx mjs cjs mts cts vue svelte json
  目录(p) → p/index.EXTS，再 package.json 的 module/main/types
  多命中 → 排序后全部返回
```

`package_entry(dir, subpath)`：先读 `exports`（条件 `types`/`import`/`module`/`default`/`require`），
再读 `module`/`main`/`types`，最后 `src/index.*`。`.vue` 包导出优先返回源码而不是 `.d.ts`。

### B5 pnpm workspace

这是本次新增的重点：

1. **发现 workspace**：从文件目录向上找最近的 `pnpm-workspace.yaml`（也接受根 `package.json` 的
   `workspaces`）；解析 `packages:` 的 glob（支持 `!` 排除）。到 workspace 根即停止。
2. **包映射**：展开 glob，跳过 `node_modules`/`.git`/`dist`，读每个 `package.json` 的 `name`；
   得到 `name → dir`。缓存，`pnpm-workspace.yaml` / `package.json` 变更时失效。
3. **解析**：`@acme/ui`（含 `@acme/ui/styles.css` 子路径）→ 包目录 → 上面的 `package_entry`。
4. **符号链接**：`node_modules/@acme/ui` 通常是 symlink；命中后 `canonicalize` 返回真实路径，
   去重，避免同一文件两个身份。LSP 仍可能用链接路径，跳转以真实路径为准。
5. **tsconfig 按包**：每个包用自己的 `tsconfig.json`；`extends` 相对声明它的文件解析，
   `baseUrl`/`paths` 记录其来源目录（TS 规则：相对声明该选项的配置）。
6. **配置就近**：vite 配置从文件目录向上找，最近者优先；包内没有就用 workspace 根的。
7. **监听**：`pnpm-workspace.yaml`、`package.json`、`tsconfig*.json`、`vite.config.*` 变化时清缓存。
8. **多根窗口**：workspace 包目录作为内容根，参与索引与搜索（`.vue`/`.tsx` 的符号）。

### B6 Vue 2

**重要发现**：`@vue/language-server` 从 3.1 起移除了 Vue 2 与 Class Component 支持
（[language-tools #5455](https://github.com/vuejs/language-tools/discussions/5455)），而本仓库的
安装器装的是 `latest`（`crates/ide-tools/src/registry.rs:314`，测试断言 `@vue/language-server@latest`）。
因此**安装器的 latest 一旦是 3.1+，Vue 2 的 `.vue` 语义跳转就不可用**——这正是“支持 Vue 2”要解决的核心问题。

策略（按优先级）：

1. **文件级跳转不依赖语言服务器**：别名/相对路径解析（B3/B4）在 Vue 2 下照常工作，
   `import Foo from '@/components/Foo.vue'` 一定能跳到文件。
2. **按项目固定语言服务器版本**：检测项目 `vue` 版本；Vue 2 项目使用 `@vue/language-server@3.0.x`
   （最后一个支持 Vue 2 的版本）。需要在工具安装器里支持“按项目选版本/并列安装”，否则只能全局二选一。
3. **内置 SFC 兜底（可选，P2）**：没有可用语言服务器时，解析 `<script>` 的 Options API
   （`components: { Foo }`）与模板标签，`<Foo/>` → `Foo.vue`。工作量 4–6 天，
   只在 Vue 2.6 必须完整支持时才做。
4. Vue 2.7 也可走 Volar 3.0（含 `<script setup>`）；Vue 2.6 建议走第 2 或第 3 条。

其余：

- **Vue CLI 默认**：`@` → `src`；`vue.config.js` 的 `configureWebpack.resolve.alias`（对象/数组）
  静态解析；`chainWebpack` 支持 `config.resolve.alias.set('@', path.resolve(__dirname, 'src'))`
  这类简单写法。
- **jsconfig.json**：Vue 2 + VS Code 常见 `compilerOptions.baseUrl/paths`，走 tsconfig 解析器。
- **Nuxt 2**：`~`/`@` → srcDir，`~~`/`@@` → rootDir。

### B7 React 18

- CRA：`jsconfig.json` 的 `baseUrl: "src"` + `paths`；Vite：tsconfig + vite alias；Next.js：`baseUrl`。
- 省略扩展名与目录 index：`./Button` → `Button.tsx`/`Button.jsx`/`Button/index.tsx`。
- JSX 标签 `<Button/>`：LSP 优先；语法回退补 `jsx_opening_element` / `jsx_self_closing_element`
  的组件角色，按名字查索引符号。
- `React.lazy(() => import('@/x'))`、`import('@/x')`：按模块引用处理。
- `memo` / `forwardRef` 包装的组件不改变名字，沿用现有索引。

### B8 中文路径（别名里带中文）

- `looksLikePath` 的裸文件名判定改用 Unicode（`\p{L}\p{N}`），支持 `"首页.vue"`、`"说明.md"`。
- 去掉路径尾部的全角标点（`。，、；：！？）】”’`）。
- 解析器候选路径同时尝试 NFC / NFD（macOS 文件名常为 NFD），返回真实路径。
- `~` / `#` 开头的说明符不再在 `is_file_reference` 里一刀切，交给模块解析按别名/`imports` 判断。

### B9 缓存

- 每个项目一个 `ModuleResolver`：`HashMap<目录, Arc<Config>>`，存配置文件的 `(mtime, len)`。
- 使用时 `stat` 校验，变了重解析；`fsChanged` 命中已知配置文件名时直接清空。
- 单次解析只做几次 `stat`，不扫目录。

### B10 IPC 与前端

新增（`crates/ide-host/src/ops/nav.rs`）：

| 命令                                 | 返回                | 用途               |
| ------------------------------------ | ------------------- | ------------------ |
| `fs_resolve_module(from, specifier)` | `Vec<ModuleTarget>` | 跳转、hover 探测   |
| `module_aliases(from)`               | `Vec<AliasInfo>`    | 补全前缀、失败提示 |

前端：

- `links.ts` 增加 `moduleReferenceAt()`（`import … from "x"`、`export … from "x"`、`require()`、
  `import()`、`new URL(…, import.meta.url)`），`Reference` 增加 `module` 类型。
- `referenceAt()` 在 LSP 无结果时调用 `fs_resolve_module`；多目标复用现有选择器。
- `probeNavigation()` 同步识别 module 引用，解析成功即显示 ⌘-hover 下划线。
- `fileReferences.ts` 在 LSP 不可用或已输入别名前缀时按 `module_aliases` 补全。
- 失败提示带原因：`Cannot resolve '@/x'（tsconfig paths / vite resolve.alias 均未命中 '@'）`。

## 5. 改动清单

| 文件                                                  | 改动                                           |
| ----------------------------------------------------- | ---------------------------------------------- |
| `crates/ide-workspace/src/module_refs/*`（新）        | 别名、workspace、包入口、缓存                  |
| `crates/ide-workspace/src/file_refs.rs`               | 扩展名探测、NFC/NFD、`~`/`#` 交给模块解析      |
| `crates/ide-host/src/ops/nav.rs`                      | `fs_resolve_module`、`module_aliases`          |
| `crates/ide-core/src/settings.rs`                     | `pathAliases` 设置、`locale` 默认改 `system`   |
| `crates/ide-language/src/syntax/*`                    | 无（本期不做 Vue SFC 语法层）                  |
| `src/features/settings/model/schema.ts`               | `LocaleId` 加 `system`、默认值、校验           |
| `src/features/settings/pages/behavior.tsx`            | 语言选项加“跟随系统”                           |
| `src/platform/i18n.ts`                                | `resolveLocale` 矩阵、`lang` 单点设置          |
| `src/features/app/locale.ts`、`src/main.tsx`          | `primeLocale()`，首屏前应用                    |
| `src/features/settings/runtime.ts`                    | 删除重复的 `root.lang`                         |
| `src/styles/tokens.css`                               | CJK 字体回退                                   |
| `src/editor/lsp/{links,navigation,fileReferences}.ts` | module 引用、跳转顺序、补全                    |
| `src/bindings.ts`                                     | `pnpm gen:bindings`                            |
| `src/platform/locales/text/zh.json`                   | 新增文案（“跟随系统”等）                       |
| `fixtures/pnpm-monorepo/*`                            | workspace fixture（Vue 2 + React 18 + 共享包） |
| `e2e/editor.spec.ts`                                  | 跳转用例                                       |

## 6. 里程碑

| 阶段 | 内容                                                                                 | 工期（1 人） |
| ---- | ------------------------------------------------------------------------------------ | ------------ |
| M1   | 系统语言：`system` 选项、启动时序、lang、字体、文案守卫                              | 1.5–2 天     |
| M2   | 解析器 P0：tsconfig paths/baseUrl、相对路径与扩展名、IPC、前端 module 引用、中文路径 | 3–4 天       |
| M3   | P1：vite/vue.config.js/webpack 别名、pnpm workspace、设置项、补全与失败提示          | 3–4 天       |
| M4   | P2：Vue 2 细节（Vue CLI、`components` 注册）、React 18 JSX 回退                      | 2–3 天       |

合计 10–13 人日；M1 可单独上线，M2 覆盖大多数别名场景。

## 7. 测试

- **系统语言**：`resolveLocale` 矩阵（`system`/`zh-CN`/`zh-Hans`/`zh-TW`/`en-US`/`fr`）；
  `primeLocale` 首屏顺序；`lang` 属性；`zh.json`/`en.json` key 对齐、无英文占位；菜单标签重推。
- **解析器**（Rust 单测）：tsconfig `extends`/`paths`/`baseUrl`；vite 对象/数组/`path.resolve`/
  `new URL`；vue.config.js；workspace 包入口与 `exports`；扩展名与 `index.*`；NFC/NFD；
  `@scope/pkg` 不误命中 `@`；`?raw` 剥离。
- **集成**：host op 返回多候选与 `via`；缓存失效。
- **e2e**：`fixtures/pnpm-monorepo`（`apps/web-vue2`、`apps/admin-react18`、`packages/ui`、
  根 `tsconfig.base.json`、每包 vite 配置、中文目录）验证：
  - `@/components/Foo.vue`、`@acme/ui`、`@acme/ui/styles.css`、`@shared/utils`；
  - 省略扩展名、目录 index、中文文件名；
  - 卸载/禁用语言服务器后仍可跳到文件（LSP 兜底路径）。
- **手工**：中文界面逐窗口走查；中文 IME 下快捷键。

## 8. 风险与对策

| 风险                   | 影响                                           | 对策                                                               |
| ---------------------- | ---------------------------------------------- | ------------------------------------------------------------------ |
| Vue 2 语言服务器版本   | 3.1+ 已移除 Vue 2 支持，Vue 2 项目语义跳转失效 | 为 Vue 2 项目固定 3.0.x；文件级跳转由解析器兜底；必要时做 SFC 兜底 |
| 静态解析 JS 配置不完整 | 动态别名漏                                     | 设置项兜底 + 失败提示 + UI 显示 `via`                              |
| pnpm 符号链接          | 同一文件两个路径                               | canonicalize + 去重                                                |
| monorepo 配置优先级    | 取错 vite/tsconfig                             | 就近优先，根配置兜底，测试覆盖                                     |
| 首屏等待               | 启动变慢                                       | 测量 `primeLocale` 耗时；`zh.json` 静态化，`text/zh.json` 懒加载   |
| 中文字体差异           | 行高/字重不齐                                  | CJK 回退栈 + 截图对比                                              |

## 9. 待确认

1. **Vue 2 是 2.6 还是 2.7？** 能否接受为 Vue 2 项目安装/固定 `@vue/language-server@3.0.x`（当前装的是 latest，已移除 Vue 2 支持）？
2. **需要繁体中文（zh-Hant）吗？** 目前只有简体目录，`zh-TW` 会显示简体。
3. **React 18 的构建工具**：Vite / CRA / Next.js，优先级如何？
4. 默认“跟随系统”是否可接受（中文系统新装即中文）？

## 10. 实现进展（2026-11-02）

已在工作区落地（未提交）：

| 模块       | 内容                                                                                                                                                                                                                                                                                                                 | 验证                                                                                        |
| ---------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------- |
| 系统语言   | `system` 选项与默认值、`primeLocale()` 首屏前应用、`lang` 单点维护、CJK 字体回退                                                                                                                                                                                                                                     | `i18n.test` / `staging.test` / `textCatalog.test` 通过，`tsrs` 通过                         |
| 模块解析器 | `crates/ide-workspace/src/module_refs.rs`：相对路径 + 扩展名/index、tsconfig `paths`/`baseUrl`/`extends`、vite/vitest/vue/webpack `resolve.alias`（对象/数组/`alias.set`）、pnpm workspace、`node_modules` 包（含 pnpm symlink）、`package.json#imports`、Nuxt/SvelteKit/Vue CLI 默认别名、设置项 `pathAliases` 兜底 | 13 个 Rust 单测                                                                             |
| IPC        | `fs_resolve_module`（host + Tauri command + 远程 dispatcher），`pnpm gen:bindings`                                                                                                                                                                                                                                   | 绑定已生成                                                                                  |
| npm 依赖   | `node_modules` 包解析（`types`/`module`/`main`/`exports`、子路径、pnpm symlink 归一化）                                                                                                                                                                                                                              | 对 `product_onething_efficient` 实测：`@tong/assistant` → `.pnpm/.../dist/types/index.d.ts` |
| 前端       | `moduleReferenceAt`（import/export/require/import()）、`resolveModule`、LSP 之后的跳转回退、⌘-hover 探测、多目标选择器、Settings › TypeScript › Path aliases                                                                                                                                                         | `links.test` 8 项、`vitest src/editor/lsp` 380 项、`tsrs` 通过                              |

尚未做（按优先级）：

1. 别名补全（`fileReferences.ts`）；
2. Vue 2：按项目固定 `@vue/language-server@3.0.x`（或 SFC 兜底）；
3. monorepo 的 `**` glob 完整支持；
4. e2e fixture（`fixtures/pnpm-monorepo`）与手工 QA 清单。
