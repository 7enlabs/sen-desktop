// Entry point of packages/core. SDK move (tin 9328/9459): the Pi SDK is
// out of this package — no Pi import lives here anymore. The model runs
// in the backend through the CLI core + stream-json bridge. This package
// keeps the session statechart, raw log, execution gate, token
// economizer, and paths.
export { SessionManager, canTransition } from "./session.ts";
export type { SessionState } from "./session.ts";
export { RawLog } from "./raw-log.ts";
export type { RawEvent } from "./raw-log.ts";
export { GATE_ORDER, assertGateOrder, executeThroughGate } from "./execution-gate.ts";
export type { GateStep } from "./execution-gate.ts";
export {
  ActivityLog,
  ApprovalStore,
  ASK_FIRST_TOOLS,
  AUTO_TOOLS,
  NEVER_TOOLS,
  classifyTool,
  defaultToolPolicy,
  describeTier,
  evaluateToolCall,
  gateToolCall,
  validateToolPolicy,
} from "./approval-gate.ts";
export type {
  ActivityEntry,
  ActivityResult,
  Approval,
  ApprovalTier,
  GateDecision,
  GateEvaluation,
  ToolCallInput,
  ToolPolicy,
} from "./approval-gate.ts";
export { TOKEN_ECONOMIZER_MAX_REDIRECT, TOKEN_ECONOMIZER_LONG_TURN_RATIO, shouldStopTurn } from "./token-economizer.ts";
export type { TurnSignal } from "./token-economizer.ts";
export { setRootDir, resolveUnderRoot } from "./paths.ts";
export {
  DECISION_MARKERS,
  buildMorningBrief,
  emailNeedsDecision,
  renderMorningBriefText,
  validateEmails,
  validateEvents,
} from "./morning-brief.ts";
export type {
  CalendarEvent,
  DeadlineItem,
  EmailItem,
  MorningBrief,
  BuildMorningBriefInput,
  ScheduleConflict,
} from "./morning-brief.ts";
export {
  applyDecision,
  idempotencyKeyFor,
  proposalIdForEmail,
  proposeDecisions,
  toolNameForAction,
} from "./proactive-inbox.ts";
export type {
  ApplyDecisionInput,
  ApplyDecisionResult,
  DecisionAction,
  DecisionProposal,
  ProposalStatus,
} from "./proactive-inbox.ts";
export {
  HttpMcpTransport,
  McpClient,
  StdioMcpTransport,
  buildDelegateArgv,
  buildDelegatePrompt,
  connectHttpMcp,
  connectStdioMcp,
  delegateToolCallToCliCore,
  executeMcpTool,
  isHttpServerConfig,
  mcpConfigFileJson,
  validateMcpServerConfig,
  writeMcpConfigFile,
  MCP_CLIENT_NAME,
  MCP_CLIENT_VERSION,
  MCP_DEFAULT_TIMEOUT_MS,
  MCP_DELEGATE_TIMEOUT_MS,
  MCP_PROTOCOL_VERSION,
} from "./mcp-client.ts";
export type {
  CliDelegateInput,
  CliDelegateResult,
  McpClientOptions,
  McpGateContext,
  McpServerConfig,
  McpTool,
  McpToolCallInput,
  McpToolResult,
  McpTransport,
} from "./mcp-client.ts";
export {
  SkillRegistry,
  parseSkillFile,
  skillBody,
  validateSkill,
  SKILL_NAME_PATTERN,
} from "./skill-registry.ts";
export type { ScanReport, Skill, SkillFile } from "./skill-registry.ts";
