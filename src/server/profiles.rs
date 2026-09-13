//! Profiles API: list, update, create, and import profiles.
//!
//! Profile definitions AND runtime state live in
//! `{data_dir}/config/profiles.yml` - the single source of truth (the legacy
//! `profiles/<name>/config.json` stays on disk for backward compat but is
//! NOT read for resolution). The profile NAME is the yml key - the stable
//! identifier used everywhere (`threads.profile`, channels.yml `profile:`,
//! kanban boards/tasks `profile:`, dashboard profile selects).
//!
//! - `GET /profiles`        : list YAML-declared profiles (bare array, the
//!   dashboard `apiGet<ProfileData[]>` shape)
//! - `POST /profiles`       : create a profile (upsert into profiles.yml)
//! - `PATCH /profiles/{id}` : update fields (provider / model / plan /
//!   template / allowed_tools), persisted atomically to profiles.yml
//! - `POST /profiles/import`: import an external `profiles.yml`-structured
//!   document (raw YAML body OR JSON `{"yaml": "..."}`) and merge it into
//!   `{data_dir}/config/profiles.yml`
//!
//! IMPORT MERGE POLICY: imported entries OVERWRITE existing entries with the
//! same name (upsert semantics) - consistent with the channels import
//! precedent, where every imported channel is PATCHed/upserted into
//! channels.yml. The whole document is validated BEFORE the atomic save; on
//! any validation/parse error nothing is written. The response lists which
//! names were newly imported and which were updated.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, patch, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::{deserialize_double_option, err_json, ok_json, AppState};
use crate::profiles_yaml::{validate_profile, ProfilesFile};

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn profiles_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/profiles", get(list_profiles_handler))
        .route("/profiles", post(create_profile_handler))
        .route("/profiles/import", post(import_profiles_handler))
        .route("/profiles/{id}", patch(update_profile_handler))
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Profile API view: the yml declaration plus the profile's filesystem
/// skills (`profiles/<name>/skills/*.md`) for the dashboard. Field names are
/// bare (`provider`/`model`/`plan`/`template`/`allowed_tools`).
#[derive(Debug, Clone, Serialize)]
pub struct ProfileEntry {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub toolset: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
}

impl ProfileEntry {
    fn from_def(name: &str, def: &crate::profiles_yaml::ProfileDef, data_dir: &str) -> Self {
        Self {
            name: name.to_string(),
            provider: def.provider.clone(),
            model: def.model.clone(),
            plan: def.plan,
            template: def.template.clone(),
            toolset: def.toolset.clone(),
            skills: list_skills(data_dir, name),
        }
    }
}

/// Body of the create-profile endpoint (dashboard "+ Create Profile").
#[derive(Debug, Deserialize)]
struct CreateProfileRequest {
    name: String,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

/// Body of the import endpoint. The external document may arrive as a JSON
/// `{"yaml": "..."}` payload (dashboard import modal) or as raw YAML text.
#[derive(Debug, Deserialize, Default)]
struct ImportRequest {
    #[serde(default)]
    yaml: Option<String>,
}

/// Fields a PATCH may update.
///
/// Every field is TRI-STATE: the key absent leaves the stored value unchanged,
/// an explicit JSON `null` clears it to None (so the resolution chain falls
/// through), and a value sets it. Empty / blank strings clear too - a
/// back-compat alias for `null` used by CLI callers and by the dashboard
/// provider select, whose "Default/None" option carries an empty value.
///
/// Before this was tri-state, `{"provider": null}` was indistinguishable from
/// an ABSENT key, so "clear the provider back to Default" from the dashboard
/// was silently ignored and the previous provider stayed in effect.
#[derive(Debug, Deserialize, Default)]
struct UpdateProfileRequest {
    #[serde(default, deserialize_with = "deserialize_double_option")]
    provider: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    model: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    plan: Option<Option<bool>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    template: Option<Option<String>>,
    /// Tri-state toolset id: absent = leave unchanged; `null` / `""` = clear to
    /// UNDEFINED (no profile-level toolset); a value = this toolset id.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub toolset: Option<Option<String>>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// `profiles/<name>/skills/*.md` on disk (profile FILES live in the profile
/// dir; the declaration lives in profiles.yml).
fn list_skills(data_dir: &str, name: &str) -> Vec<String> {
    let dir = std::path::Path::new(data_dir)
        .join("profiles")
        .join(name)
        .join("skills");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
        .filter(|s| s.ends_with(".md"))
        .collect();
    names.sort();
    names
}

/// Normalize an optional field from the API: empty/whitespace → None.
fn clean_opt(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Apply a tri-state PATCH body to a stored definition (see
/// [`UpdateProfileRequest`]): absent = leave unchanged, inner `None` = clear,
/// inner `Some(value)` = set (blank strings clear, via [`clean_opt`]).
///
/// Extracted from the handler so the clear/set/unchanged contract is directly
/// unit-testable without spinning up the HTTP router.
fn apply_update(def: &mut crate::profiles_yaml::ProfileDef, req: &UpdateProfileRequest) {
    if let Some(v) = req.provider.clone() {
        def.provider = clean_opt(v);
    }
    if let Some(v) = req.model.clone() {
        def.model = clean_opt(v);
    }
    if let Some(v) = req.plan {
        def.plan = v;
    }
    if let Some(v) = req.template.clone() {
        def.template = clean_opt(v);
    }
    if let Some(v) = req.toolset.clone() {
        def.toolset = clean_opt(v);
    }
}

/// Extract the YAML document from an import request body: a JSON
/// `{"yaml": "..."}` payload or the raw body itself (YAML text).
fn yaml_from_body(body: &str) -> Result<String, String> {
    let trimmed = body.trim();
    if trimmed.starts_with('{') {
        if let Ok(req) = serde_json::from_str::<ImportRequest>(trimmed) {
            if let Some(y) = req.yaml.filter(|s| !s.trim().is_empty()) {
                return Ok(y);
            }
        }
    }
    if trimmed.is_empty() {
        return Err(
            "empty import body: expected a profiles.yml-structured YAML document".to_string(),
        );
    }
    Ok(trimmed.to_string())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /profiles - YAML-declared profiles, sorted by name. Returns a BARE
/// array (dashboard consumes it as `ProfileData[]`), mirroring GET /channels.
async fn list_profiles_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<ProfileEntry>>, (StatusCode, Json<serde_json::Value>)> {
    let file = match crate::profiles_yaml::load_profiles_from(&state.data_dir) {
        Ok(f) => f,
        Err(e) => return Err(err_json(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())),
    };
    let mut entries: Vec<ProfileEntry> = file
        .profiles
        .iter()
        .map(|(name, def)| ProfileEntry::from_def(name, def, &state.data_dir))
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Json(entries))
}

/// POST /profiles - create (upsert) a profile from `{name, provider, model}`.
async fn create_profile_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateProfileRequest>,
) -> impl IntoResponse {
    let name = req.name.trim().to_string();
    let def = crate::profiles_yaml::ProfileDef {
        provider: clean_opt(req.provider),
        model: clean_opt(req.model),
        ..Default::default()
    };
    if let Err(e) = validate_profile(&name, &def) {
        return err_json(StatusCode::BAD_REQUEST, &e);
    }
    match crate::profiles_yaml::update_profile_in(&state.data_dir, &name, |_existing| Ok(def)) {
        Ok(def) => ok_json(ProfileEntry::from_def(&name, &def, &state.data_dir)),
        Err(e) => err_json(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

/// PATCH /profiles/{id} - update one or more bare fields.
///
/// Tri-state per field: an ABSENT key leaves it unchanged, an explicit JSON
/// `null` (or an empty/blank string, back-compat) clears
/// provider/model/plan/template/toolset to None so the resolution chain falls
/// through, and a value stores it. Response body = the updated entry, so a
/// caller can assert the stored state without a second GET.
async fn update_profile_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateProfileRequest>,
) -> impl IntoResponse {
    let name = id.trim().to_string();
    if name.is_empty() {
        return err_json(StatusCode::BAD_REQUEST, "profile name must not be empty");
    }
    let result = crate::profiles_yaml::update_profile_in(&state.data_dir, &name, |existing| {
        let mut def = existing.cloned().unwrap_or_default();
        apply_update(&mut def, &req);
        Ok(def)
    });
    match result {
        Ok(def) => ok_json(ProfileEntry::from_def(&name, &def, &state.data_dir)),
        Err(e) => err_json(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

/// POST /profiles/import - merge an external profiles.yml-structured
/// document into `{data_dir}/config/profiles.yml`.
///
/// Body: raw YAML text (`profiles:` top-level) OR JSON `{"yaml": "..."}`.
/// Merge policy: existing entries with the same name are OVERWRITTEN
/// (upsert - same as the channels import precedent); new names are added.
/// The whole document is validated BEFORE the atomic save; on error nothing
/// is written. Response: `{imported: [...], updated: [...]}`.
async fn import_profiles_handler(
    State(state): State<Arc<AppState>>,
    body: String,
) -> impl IntoResponse {
    let yaml_text = match yaml_from_body(&body) {
        Ok(y) => y,
        Err(e) => return err_json(StatusCode::BAD_REQUEST, &e),
    };
    let parsed: ProfilesFile = match serde_yaml::from_str(&yaml_text) {
        Ok(p) => p,
        Err(e) => {
            return err_json(
                StatusCode::BAD_REQUEST,
                &format!("Failed to parse profiles YAML: {}", e),
            )
        }
    };
    if parsed.profiles.is_empty() {
        return err_json(
            StatusCode::BAD_REQUEST,
            "No profiles found: expected a top-level `profiles:` map",
        );
    }
    // Validate every entry BEFORE persisting anything.
    for (name, def) in &parsed.profiles {
        if let Err(e) = validate_profile(name, def) {
            return err_json(StatusCode::BAD_REQUEST, &e);
        }
    }
    match crate::profiles_yaml::merge_profiles_file_in(&state.data_dir, &parsed) {
        Ok((added, updated)) => {
            let mut message = format!("Imported {} profile(s)", added.len() + updated.len());
            if !added.is_empty() {
                message.push_str(&format!("; new: {}", added.join(", ")));
            }
            if !updated.is_empty() {
                message.push_str(&format!("; updated: {}", updated.join(", ")));
            }
            ok_json(serde_json::json!({
                "imported": added,
                "updated": updated,
                "message": message,
            }))
        }
        Err(e) => err_json(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_from_body_accepts_raw_yaml_and_json() {
        let raw = "profiles:\n  omni:\n    plan: false\n";
        // Raw YAML is returned trimmed (trailing whitespace/newline dropped).
        assert_eq!(yaml_from_body(raw).unwrap(), raw.trim());
        let json = format!(r#"{{"yaml": "{}"}}"#, raw.replace('\n', "\\n"));
        assert_eq!(yaml_from_body(&json).unwrap(), raw);
        assert!(yaml_from_body("").is_err());
        assert!(yaml_from_body("   ").is_err());
    }

    #[test]
    fn import_parse_and_validation() {
        // Valid document parses into a ProfilesFile with the expected entries.
        let yaml = r#"
profiles:
  omni:
    toolset: dev_set
  research:
    provider: opencode-go
    model: deepseek-v4-flash
    plan: true
"#;
        let parsed: ProfilesFile = serde_yaml::from_str(yaml).expect("parse");
        assert_eq!(parsed.profiles.len(), 2);
        for (name, def) in &parsed.profiles {
            validate_profile(name, def).expect("valid entry");
        }
        // An entry with a blank toolset id fails validation (loud, pre-write).
        let bad = r#"
profiles:
  broken:
    toolset: ""
"#;
        let parsed_bad: ProfilesFile = serde_yaml::from_str(bad).expect("parse");
        let name = parsed_bad.profiles.keys().next().unwrap().clone();
        let def = &parsed_bad.profiles[&name];
        assert!(validate_profile(&name, def).is_err());
    }

    #[test]
    fn clean_opt_normalizes_empty() {
        assert_eq!(clean_opt(Some("  ".to_string())), None);
        assert_eq!(
            clean_opt(Some("opencode-go".to_string())).as_deref(),
            Some("opencode-go")
        );
        assert_eq!(clean_opt(None), None);
    }

    fn populated_def() -> crate::profiles_yaml::ProfileDef {
        crate::profiles_yaml::ProfileDef {
            provider: Some("opencode-go".to_string()),
            model: Some("deepseek-v4-flash".to_string()),
            plan: Some(true),
            template: Some("dev-development".to_string()),
            toolset: Some("dev_set".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn patch_absent_keys_leave_values_unchanged() {
        let mut def = populated_def();
        let req: UpdateProfileRequest = serde_json::from_str("{}").unwrap();
        apply_update(&mut def, &req);
        assert_eq!(def.provider.as_deref(), Some("opencode-go"));
        assert_eq!(def.model.as_deref(), Some("deepseek-v4-flash"));
        assert_eq!(def.plan, Some(true));
        assert_eq!(def.template.as_deref(), Some("dev-development"));
        assert_eq!(def.toolset.as_deref(), Some("dev_set"));
    }

    #[test]
    fn patch_explicit_null_clears_to_none() {
        let mut def = populated_def();
        let req: UpdateProfileRequest = serde_json::from_str(
            r#"{"provider":null,"model":null,"plan":null,"template":null,"toolset":null}"#,
        )
        .unwrap();
        apply_update(&mut def, &req);
        assert_eq!(def.provider, None, "explicit null must clear the provider");
        assert_eq!(def.model, None);
        assert_eq!(def.plan, None);
        assert_eq!(def.template, None);
        assert_eq!(def.toolset, None);
    }

    #[test]
    fn patch_empty_string_clears_and_value_sets() {
        let mut def = populated_def();
        let req: UpdateProfileRequest =
            serde_json::from_str(r#"{"provider":"","model":"   ","toolset":""}"#).unwrap();
        apply_update(&mut def, &req);
        assert_eq!(def.provider, None, "empty string must clear the provider");
        assert_eq!(def.model, None);
        assert_eq!(def.toolset, None);
        // Explicit values are stored (and trimmed).
        let req: UpdateProfileRequest =
            serde_json::from_str(r#"{"provider":" deepseek ","model":"deepseek-v4-flash"}"#)
                .unwrap();
        apply_update(&mut def, &req);
        assert_eq!(def.provider.as_deref(), Some("deepseek"));
        assert_eq!(def.model.as_deref(), Some("deepseek-v4-flash"));
    }
}
