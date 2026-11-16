# Sen P0-5 - MCP client + SKILL.md registry

Status: PRODUCER BUNDLE, 16/11/2026. Verification by a verifier
other than the producer still pending. No merge, no push, no PR.

## OBSERVABLE_SUCCESS

- MCP client kết nối được MCP server (stdio và Streamable HTTP),
  liệt kê được tool, gọi được tool và nhận kết quả thật: với server
  giả trong test, `add(19, 23)` trả về `42`.
- Registry quét thư mục skill thật trên đĩa, đọc được file
  `SKILL.md` thật, và chọn đúng skill cho yêu cầu tiếng Việt:
  `cho tôi bản tin sáng hôm nay` chọn `morning-brief`.
- Mọi tool call có side-effect đi qua cổng duyệt P0-2
  (default-deny): không có quy tắc cho phép thì bị từ chối, có quy
  tắc mà chưa có duyệt thì chờ duyệt, công cụ tầng never thì luôn
  bị từ chối dù có quy tắc và duyệt.
- Không có runtime agent thứ hai: Sen không tự xây vòng lặp agent
  hay subagent. Tool call cần chạy trong một lượt agent được giao
  cho CLI core của ZenCore (`zen -p --mcp-config ...`), CLI core
  mới là bên sở hữu vòng lặp agent, giới hạn subagent, và vòng đời
  MCP server.

## Design

### MCP client

`packages/core/src/mcp-client.ts` - pure TS, chỉ dùng built-in
của Node (`node:child_process`, `node:fs`, `node:os`,
`node:path`, `node:crypto`, `fetch` toàn cục). Không thêm
dependency, không gọi model, không cầm key.

- Giao thức: JSON-RPC 2.0. Client giữ id đơn điệu cho mỗi client,
  timeout cho mỗi request (mặc định 30 giây), bắt tay
  `initialize` với `protocolVersion: "2024-11-05"`, rồi gửi
  notification `notifications/initialized`.
- Transport là một interface (`McpTransport`): `StdioMcpTransport`
  spawn server, JSON phân tách bằng dòng mới, stderr chỉ giữ đuôi
  8 KB; `HttpMcpTransport` POST một JSON-RPC object và đọc trả lời
  dạng JSON thuần hoặc SSE (`data:`). Một POST cho một request,
  không giữ stream GET mở.
- Lỗi theo kiểu FAIL-loud, dạng `MÃ: thông điệp`: `MCP_TIMEOUT`
  (không có trả lời trong hạn), `MCP_TOOL_ERROR` (tools/call bị
  server từ chối bằng lỗi JSON-RPC), `MCP_PROTOCOL_ERROR` (lỗi
  giao thức khác), `MCP_SERVER_EXITED` (server đóng transport),
  `MCP_SERVER_CLOSED`, `MCP_CONFIG_INVALID`, `MCP_VALIDATION`,
  `MCP_NOT_INITIALIZED`, `MCP_CLIENT_CLOSED`.
- Server gửi request về client (có method và id) được trả lời
  `-32601 Method not found`: client này không có method phía
  server. Notification từ server được bỏ qua. Dòng stdout không
  parse được JSON thì bỏ qua, vì không gắn được với request nào.
- Kết quả tool có `isError: true` vẫn là một lần gọi đã hoàn tất:
  hàm trả về kết quả, không throw. Người gọi đọc `isError`.

### Cổng duyệt

`executeMcpTool(client, gate, input)`:

1. Phân tầng tool theo tên MCP bằng bảng P0-2 (`classifyTool`):
   tên chưa phân loại là ask-first, không bao giờ auto.
2. Đánh giá qua `gateToolCall` (ghi log đúng một lần khi có log).
   Không allow thì throw mã P0-2 (`TOOL_POLICY_DENIED` /
   `APPROVAL_REQUIRED` / `APPROVAL_NEVER_DENIED`) và tool không
   bao giờ chạy.
3. Allow thì gọi `client.callTool`. Allow nghĩa là được phép, chưa
   phải đã thực thi: server MCP vẫn là bên chạy tool.

### Tái dùng runtime ZenCore - không runtime thứ hai

Sen không có vòng lặp agent, không có subagent, không có spawn
agent riêng. Hai đường chạy tool:

| Đường | Ai chạy tool | Khi nào dùng |
| --- | --- | --- |
| Trực tiếp | Server MCP, qua `McpClient` + cổng P0-2 | Desktop tự gọi một tool MCP cụ thể, không cần lượt agent |
| Ủy quyền | CLI core của ZenCore | Tool cần chạy trong một lượt agent (model quyết định, có thể gọi nhiều tool, có thể mở subagent) |

Đường ủy quyền là `delegateToolCallToCliCore` trong cùng file:

- Ghi config server ra file tạm dạng `{"mcpServers": {name:
  config}}`, đúng dạng `zen-mcp load_mcp_configs_scoped` đọc
  (ZenCore `crates/zen-mcp/src/config.rs`, checkout
  `~/workspace/cli-core-main` tại `8ac73fe`, chỉ đọc).
- Chạy `zen -p --output-format stream-json --bare
  --setting-sources "" --mcp-config <file> [--model ...]
  [--base-url ...] [--session-id <id> | -r <id>] <prompt>`, đúng
  cờ của cầu nối CLI đã có (`cli_spawn.rs`). Prompt nêu tên
  server, tên tool, và đúng JSON arguments, yêu cầu chỉ trả về
  kết quả tool.
- Đọc bản ghi `result` của stream-json (trường `result`, cờ
  `is_error`), đúng dạng `stream_json.rs` dịch. Không có bản ghi
  result trong hạn (mặc định 120 giây, bằng `TURN_TIMEOUT` của
  cầu nối) thì kill và báo `CLI_TURN_TIMEOUT`; result lỗi thì
  báo `CLI_DELEGATE_ERROR`; không có result thì `CLI_NO_RESULT`;
  không spawn được `zen` thì `CLI_SPAWN_FAILED`. stdin là null
  như cầu nối, để CLI không chờ stdin. File config tạm bị xóa
  trong khối finally.
- Vòng lặp agent, giới hạn subagent (`SubAgentExecutor`,
  `max_depth` mặc định 1, `crates/zen-agent/src/subagent.rs`),
  kiểm tra quyền, và vòng đời MCP server (`McpManager::start`,
  `crates/zen-mcp/src/manager.rs`) đều thuộc về CLI core. Sen
  chỉ hỏi và đọc kết quả.

Bằng chứng không có runtime thứ hai (kiểm tra bằng grep trên
bundle này):

- Không có module nào spawn tiến trình agent ngoài hai chỗ:
  (a) `StdioMcpTransport` spawn tiến trình MCP server (transport
  giao thức, không phải agent), (b) `delegateToolCallToCliCore`
  spawn binary `zen` (runtime đã có).
- Không có vòng lặp gọi model, không có `ToolRegistry` riêng,
  không có `SubAgentExecutor` riêng, không có `max_depth` riêng.
- `grep -rn "subagent\|SubAgent\|ToolRegistry\|max_depth"
  packages/core/src/mcp-client.ts packages/core/src/skill-registry.ts`
  chỉ ra các dòng comment ghi rõ đây là thứ thuộc về CLI core.

### SKILL.md registry

`packages/core/src/skill-registry.ts` - pure TS, chỉ dùng
`node:fs` và `node:path`. Bố cục khớp CLI core
(`crates/zen-cli/src/skills.rs` + `crates/zen-ext/src/skills.rs`):
một skill một thư mục, `<skills-root>/<tên>/SKILL.md`,
frontmatter giữa hai dòng `---` với `name:` và `description:`,
phần thân sau đó. Frontmatter không đóng thì toàn bộ file là
thân, giống CLI. Hai trường riêng của Sen: `triggers:` (danh sách
hoặc chuỗi phân cách bằng dấu phẩy, dùng để chọn skill) và
`allowed-tools:` (danh sách, khớp `SkillInfo.allowed_tools` của
CLI core). Key frontmatter lạ được bỏ qua, để skill viết cho CLI
vẫn nạp được.

- `SkillRegistry.scan(root)`: đọc thư mục thật, nạp mọi
  `*/SKILL.md` con trực tiếp. Root chưa có thì quét ra rỗng, root
  không phải thư mục thì throw. File lỗi hoặc trùng tên thì ghi
  vào `failed` (đường dẫn + lý do), không throw, không giấu các
  skill còn lại.
- `matchSkill(query)`: chấm điểm - trigger xuất hiện trong yêu
  cầu +10 mỗi cái, tên skill xuất hiện +5, từ trong mô tả xuất
  hiện +1 mỗi từ (tối đa +5). Không khớp thì trả về undefined:
  yêu cầu không liên quan không được kéo skill vào.
- `skillBody(skill)`: thay `${ZEN_SKILL_DIR}` bằng thư mục skill,
  đúng như `SubAgentExecutor` của CLI làm khi đưa skill cho
  subagent, để một skill dùng chung đọc giống nhau ở hai bên.
- Registry chỉ chọn và đưa thân skill cho người gọi. Nó không
  chạy skill, không gọi model, không spawn agent. Chạy skill là
  việc của CLI core.

### Skill mẫu

`skills/morning-brief/SKILL.md` là skill thật, không placeholder:
mô tả cách xây bản tin sáng bằng `buildMorningBrief` và
`proposeDecisions` của P0-4, đi qua cổng duyệt P0-2 khi áp dụng
đề xuất, và các việc không làm (không gửi mail, không đặt lịch
khi chưa duyệt, không đoán email cần quyết ngoài cờ và marker
trong code). Trigger gồm `bản tin sáng`, `morning brief`,
`hộp thư`, `quyết định chờ duyệt`, `lịch hôm nay`.

### Nơi code nằm

- `packages/core/src/mcp-client.ts` - config + validation, hai
  transport, `McpClient`, `executeMcpTool`, delegate sang CLI
  core. Export từ `packages/core/src/index.ts`.
- `packages/core/src/skill-registry.ts` - parse frontmatter,
  `SkillRegistry`, `matchSkill`, `skillBody`. Export từ
  `packages/core/src/index.ts`.
- `skills/morning-brief/SKILL.md` - skill mẫu thật.
- Test + fixture giả: `packages/core/tests/mcp-client.test.ts`,
  `mcp-delegate.test.ts`, `skill-registry.test.ts`,
  `fixtures/mcp-fake-server.mjs` (server MCP giả qua stdio:
  echo, add, fail, slow, read_file),
  `fixtures/mcp-fake-zen.mjs` (binary `zen` giả phát bản ghi
  stream-json, có các chế độ success / error / noresult / hang).

### Giới hạn đã biết (không giấu)

1. Không có mirror Rust và không có Tauri command cho P0-5 trong
   bundle này: backend chưa có lệnh gọi MCP, frontend chưa có
   client IPC. Việc nối Tauri command là follow-up sau review,
   theo tiền lệ P0-4 (không nối `main.ts`, nối sau review).
2. MCP client không nhận subscription từ server: transport HTTP
   không giữ stream GET mở; server gửi notification thì bị bỏ
   qua; client không implement method phía server ngoài trả lời
   -32601.
3. Ủy quyền sang CLI core phụ thuộc vào `zen` trên PATH và server
   MCP mà CLI core khởi động được: nếu CLI core không thấy server
   (sai config, thiếu binary), lượt agent lỗi ở phía CLI, Sen chỉ
   trả về text lỗi đó, không tự sửa config.
4. `read_file` trong server giả chỉ là tên tool trùng với tầng
   auto của P0-2 để test đường allow không cần duyệt; nó không
   đọc file thật.
5. Chưa có lượt model thật nào chạy qua MCP server thật: bằng
   chứng gọi tool là server giả local và binary `zen` giả (xem
   NOT RUN).

## Verification skill - MCP client + SKILL.md registry

Feature map: entry `packages/core/src/mcp-client.ts` +
`packages/core/src/skill-registry.ts` +
`skills/morning-brief/SKILL.md` | actions: validate config, kết
nối stdio/HTTP, list tools, gọi tool, gọi tool qua cổng duyệt,
quét registry, chọn skill, ủy quyền sang CLI core | observable:
danh sách tool, kết quả tool thật (`add(19,23)` = `42`), mã lỗi
cổng duyệt, skill được chọn, text của bản ghi result, file config
tạm đúng dạng zen-mcp.

### 1. LAUNCH

- Worktree `~/workspace/sen-p05-mcp-client`, nhánh
  `feature/sen-mcp-client`, base HEAD `d96975a` (P0-4, PR #72).
- `git log -1` lúc chạy: xem Evidence.
- Build/chạy: `npm test` (node 24, type stripping, không có bước
  build); `npm run build --prefix apps/desktop` (vite).

### 2. DOCTOR

- Linux, node v24.22.1, không có `zen` thật trên PATH (delegate
  chỉ chứng minh bằng binary `zen` giả), không có Rust system
  lib (không chạm Rust trong bundle này - pure TS).
- Fail sớm khi: HEAD không phải `d96975a`, `node_modules` ở
  `apps/desktop` thiếu (`npm install --prefix apps/desktop`
  trước; baseline không có nó thì các test apps/desktop fail vì
  thiếu `@tauri-apps/api`).

### 3. DRIVE

| # | Lệnh chạy thật | Kết quả |
| --- | --- | --- |
| 1 | `npm test` (baseline, trước thay đổi) | 165 pass, 0 fail |
| 2 | `node --test packages/core/tests/mcp-client.test.ts packages/core/tests/skill-registry.test.ts packages/core/tests/mcp-delegate.test.ts` | 35 pass, 0 fail |
| 3 | `npm test` (sau thay đổi) | 200 pass, 0 fail |
| 4 | `npm run build --prefix apps/desktop` | vite build green, 21.80 kB js |
| 5 | `node /tmp/p05-demo.mjs` (probe độc lập, không phải test của producer) | PROBE OK (xem Evidence) |
| 6 | `git diff --check` | sạch |

### 4. EVIDENCE

- TS suite: 200 pass / 0 fail (165 baseline + 35 mới: 15 trong
  `mcp-client.test.ts`, 9 trong `mcp-delegate.test.ts`, 11 trong
  `skill-registry.test.ts`).
- Probe độc lập `/tmp/p05-demo.mjs` (đọc code thật, chạy server
  giả thật, binary `zen` giả thật), output thật:
  - `tools: echo,add,fail,slow,read_file`
  - `add(19,23) = 42 isError: false`
  - `skills: morning-brief`
  - `match 'bản tin sáng': morning-brief`
  - `delegate text: echo: {"hello":"world"} | session: s-probe-1 | exit: 0`
  - `PROBE OK`
- `git log -1` lúc bàn giao: base HEAD `d96975a` (P0-4, PR #72);
  thay đổi P0-5 nằm trong một commit trên nhánh
  `feature/sen-mcp-client` (producer bundle, xem bundle README
  trong báo cáo bàn giao).
- `git diff --check`: sạch (kiểm tra lúc bundle).
- File đổi/tạo (11 file, xem `git diff --stat` trong báo cáo):
  tạo `packages/core/src/mcp-client.ts`,
  `packages/core/src/skill-registry.ts`,
  `packages/core/tests/mcp-client.test.ts`,
  `mcp-delegate.test.ts`, `skill-registry.test.ts`,
  `fixtures/mcp-fake-server.mjs`, `fixtures/mcp-fake-zen.mjs`,
  `skills/morning-brief/SKILL.md`, `docs/mcp-client.md`; sửa
  `packages/core/src/index.ts` (chỉ thêm export).

### 5. CLEANUP

- Giữ lại: worktree (nhánh `feature/sen-mcp-client`, đã commit),
  file bundle trong `~/workspace`, probe `/tmp/p05-demo.mjs`
  (tạm, chạy lại được từ worktree).
- Không xóa gì trong repo. `apps/desktop/node_modules` và
  `apps/desktop/dist` là output build được gitignore, để nguyên.
- Evidence vẫn đọc được sau cleanup: file này + các file test.

## NOT RUN (must run before merge)

- `cargo test` trong `apps/desktop/src-tauri` và `cargo clippy`:
  NOT RUN - không chạm Rust trong bundle này, không có mirror để
  test; `cargo test` cho crate đầy đủ vẫn BLOCKED trên box này
  vì thiếu `gdk-3.0`. Chạy trên Leader VM khi mirror Tauri hạ cánh.
- Test PC (Windows): NOT RUN - box chỉ có Linux.
- CI: NOT RUN - chưa mở PR (producer bundle, không push).
- Lượt model thật qua MCP server thật: NOT RUN - chưa có lượt
  `zen` thật nào; delegate chỉ chứng minh bằng binary `zen` giả,
  client chỉ chứng minh bằng server giả local.
- Independent verification (producer khác verifier): NOT RUN -
  bundle này là việc của producer; ghế verify sau.

## Handoff

| Check | Verdict | Bằng chứng (lệnh, output, file) |
| --- | --- | --- |
| `npm test` (TS, gồm 35 mới) | PASS | 200 pass / 0 fail |
| `npm run build --prefix apps/desktop` | PASS | vite build green, 21.80 kB js |
| Rust mirror + `cargo test` trong src-tauri | NOT RUN | không chạm Rust; gdk-3.0 thiếu trên box |
| `cargo clippy` | NOT RUN | như trên |
| Test PC (Windows) | NOT RUN | box chỉ có Linux |
| CI | NOT RUN | chưa mở PR |
| Lượt model thật qua MCP server thật | NOT RUN | chỉ server giả local + `zen` giả |
| Independent review | NOT RUN | producer bundle; ghế verify |

Rủi ro còn lại: Tauri command chưa nối nên desktop chưa dùng
được MCP client từ UI - chỉ dùng được từ `packages/core`; nối
command là follow-up và cần review riêng vì nó chạm backend
enforcing. Delegate phụ thuộc `zen` trên PATH ở máy chạy; ở box
này không có `zen` thật nên đường delegate chưa từng chạy thật
với CLI core thật. Transport HTTP không giữ stream GET nên server
đẩy event chủ động sẽ không tới được client.
