// Real Tauri command handler for the brief source (Tauri
// follow-up 2, ZC Leader tin 17093). Thin wrapper only: it reads
// the token from the environment, builds the HTTP transport, and
// delegates to commands/brief_sources.rs, which owns the Gmail +
// Calendar fetch. No fetch logic lives here, and no token is
// stored, logged, or returned: BriefSourceConfig::from_env reads
// SEN_GMAIL_ACCESS_TOKEN per call, and an unset token fails
// honestly as BRIEF_SOURCE_NOT_CONFIGURED instead of fetching
// anything.
use std::time::SystemTime;

use super::brief_sources::{BriefData, BriefSource, BriefSourceConfig, ReqwestBriefHttp};
use super::session::ContractArgs;

/// sen_brief_source: fetch the morning-brief source data (Gmail
/// unread-window emails + the 7-day calendar window) and return it
/// as EmailItem / CalendarEvent JSON. The TS side builds the brief
/// from this data with buildMorningBrief; this command never
/// synthesizes a brief. A failed fetch (no token, HTTP error, bad
/// shape) is a typed error, never a partial or made-up brief.
#[tauri::command]
pub fn sen_brief_source(contract: ContractArgs) -> Result<BriefData, String> {
    super::registry::validate_contract(&contract.to_registry()).map_err(|e| e.to_wire())?;
    let config = BriefSourceConfig::from_env().map_err(|e| format!("{}: {}", e.code, e.message))?;
    let http = ReqwestBriefHttp::new()?;
    let source = BriefSource::new(http, config);
    source
        .fetch(SystemTime::now())
        .map_err(|e| format!("{}: {}", e.code, e.message))
}
