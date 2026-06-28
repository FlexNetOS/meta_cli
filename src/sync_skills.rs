//! `meta sync skills` — Skill distribution engine (upgrade-only).
//!
//! Reads `.meta/skill-registry.json` and distributes each registered skill
//! to repos whose tag set intersects the skill's `distributes_to` list.
//!
//! Modes (analogous to sync-skills.sh):
//!   pilot     → handoff + ruflo (default), first 2 canon + first 2 ai as fallback
//!   expand    → all repos tagged [ai], [orchestration], or [canon]
//!   all       → every repo in .meta.yaml with a .claude/ directory
//!   conformance → sha256 drift check only, no copies
//!
//! Upgrade logic:
//!   - If the deployed file exists and has a lower Version header → create `<name>.v2` alongside
//!   - If identical sha256 → IN SYNC (no-op)
//!   - If the deploy path does not exist yet → CREATE fresh
//!   - NEVER deletes or removes any existing file

use anyhow::{bail, Context, Result};
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Default update policy for skills with no explicit setting
fn default_update_policy() -> String {
    "always".to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
// Data structures (mirrors .meta/skill-registry.json)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct Registry {
    version: u32,
    #[allow(dead_code)]
    last_updated: String,
    skills: Vec<SkillEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SkillEntry {
    name: String,
    source: String,
    kind: String, // "skill" | "rule"
    #[serde(default)]
    distributes_to: Vec<String>,
    #[serde(rename = "update_policy", default = "default_update_policy")]
    update_policy: String, // "always" | "newer"
    #[serde(rename = "dest_dir")]
    dest_dir: String, // ".claude/skills" | ".claude/rules"
}

#[derive(Debug)]
struct ProjectEntry {
    name: String,
    #[allow(dead_code)] // remote URL; needed for registry sync cross-repo reference
    repo: String,
    tags: Vec<String>,
    path: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// CLI args
// ─────────────────────────────────────────────────────────────────────────────

/// Arguments for `meta sync skills`
#[derive(Debug)]
pub struct SyncSkillsArgs {
    pub mode: Mode,
    pub upgrade: bool,
    #[allow(dead_code)] // reserved for future use — always upgrade-only semantics
    pub force: bool,
    pub json_output: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Pilot,
    Expand,
    All,
    Conformance,
}

impl Default for Mode {
    fn default() -> Self {
        Mode::Pilot
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler (entry point)
// ─────────────────────────────────────────────────────────────────────────────

pub fn handle_sync_skills(args: SyncSkillsArgs, meta_root: &Path) -> Result<()> {
    let registry_path = meta_root.join(".meta/skill-registry.json");
    if !registry_path.exists() {
        bail!("Skill registry not found at {}", registry_path.display());
    }

    let yaml_path = meta_root.join(".meta.yaml");
    if !yaml_path.exists() {
        bail!(".meta.yaml not found at {}", yaml_path.display());
    }

    // Parse inputs
    let registry: Registry = serde_json::from_str(
        &fs::read_to_string(&registry_path)
            .with_context(|| format!("Failed to read registry at {}", registry_path.display()))?,
    )
    .with_context(|| "Registry JSON parse error — check schema")?;

    let projects = load_projects(&yaml_path)?;
    let tags_map = build_tags_map(&projects);

    // Select targets
    let targets = select_targets(&args.mode, &meta_root, &tags_map, &projects)?;

    if targets.is_empty() {
        eprintln!("No matching target repos found.");
        return Ok(());
    }

    let mut result = SyncResult::default();

    // Run sync for each target repo
    for target_name in &targets {
        let project = match projects.iter().find(|p| p.name == *target_name) {
            Some(p) => p,
            None => continue,
        };

        let target_base = compute_target_base(&meta_root, project);

        // Check .claude/ exists for non-conformance modes
        if !args.dry_run || args.mode != Mode::Conformance {
            let claude_dir = target_base.join(".claude");
            if !args.dry_run && !claude_dir.exists() {
                result.repos_skipped += 1;
                continue;
            }
        }

        let _deploy_path = match args.mode {
            Mode::Conformance => format!("(conformance check: {})", target_base.display()),
            Mode::All | Mode::Pilot | Mode::Expand => {
                format!("(deploy to: {})", target_base.display())
            }
        };

        println!(
            "{}",
            format!("--- Syncing to: {} ---", target_name)
                .green()
                .bold()
        );

        result.repos_scanned += 1;

        // For each skill, check tag match and sync
        for skill in &registry.skills {
            // Empty distribute_to = ship to every repo
            if !skill.distributes_to.is_empty() {
                let repo_tags: Vec<&str> = project.tags.iter().map(|t| t.as_str()).collect();
                let mut matched = false;
                for stag in &skill.distributes_to {
                    if repo_tags.contains(&stag.as_str()) {
                        matched = true;
                        break;
                    }
                }
                if !matched {
                    continue;
                }
            }

            result.skills_evaluated += 1;

            let source_path = meta_root.join(&skill.source);
            if !source_path.exists() {
                eprintln!(
                    "{}: registry source missing: {} (skill: {})",
                    "WARN".yellow(),
                    source_path.display(),
                    skill.name
                );
                result
                    .errors
                    .push(format!("REGISTRY_SOURCE_MISSING:{}", skill.name));
                continue;
            }

            let registry_sha = sha256_file(&source_path);

            // Build deploy path
            let (dest_subdir, deploy_filename) = if skill.dest_dir == ".claude/rules" {
                (String::new(), format!("{}.md", skill.name))
            } else {
                (skill.name.clone(), "SKILL.md".to_string())
            };

            let dest_path = target_base
                .join(&skill.dest_dir)
                .join(&dest_subdir)
                .join(&deploy_filename);

            sync_skill_to_repo(
                &skill,
                &source_path,
                registry_sha,
                &dest_path,
                &args,
                meta_root,
                &mut result,
            )?;
        }
    }

    // Output
    if args.json_output {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        output_summary(&result, args.mode, args.dry_run, args.upgrade);
    }

    // Exit code: 2 = conformance drift found but --upgrade was NOT requested
    if args.mode == Mode::Conformance && result.drifts_found > 0 && !args.upgrade {
        bail!("Exit 2: conformance drift found without --upgrade");
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Sync a single skill to a single repo
// ─────────────────────────────────────────────────────────────────────────────

fn sync_skill_to_repo(
    skill: &SkillEntry,
    source_path: &Path,
    registry_sha: String,
    dest_path: &Path,
    args: &SyncSkillsArgs,
    _meta_root: &Path,
    result: &mut SyncResult,
) -> Result<()> {
    // Helper to compare modification times
    fn mtime_newer(a: &Path, b: &Path) -> bool {
        let a_mod = a.metadata().ok().and_then(|m| m.modified().ok());
        let b_mod = b.metadata().ok().and_then(|m| m.modified().ok());
        match (a_mod, b_mod) {
            (Some(a), Some(b)) => a > b,
            _ => true, // can't compare → default to deploy
        }
    }
    if !dest_path.exists() {
        // Conformance-only: only report on existing files, not missing ones
        if args.mode == Mode::Conformance {
            return Ok(());
        }
        if args.dry_run {
            println!(
                "  {}",
                format!(
                    "CREATE:    {} (registry sha: {}...)",
                    dest_path.display(),
                    &registry_sha[..12]
                )
                .green()
            );
        } else {
            let _ = fs::create_dir_all(dest_path.parent().unwrap());
            fs::copy(source_path, dest_path)
                .with_context(|| format!("Failed to copy to {}", dest_path.display()))?;
            println!(
                "  {}",
                format!(
                    "CREATED:   {} (sha: {}...)",
                    dest_path.display(),
                    &registry_sha[..12]
                )
                .green()
            );
        }
        result.files_created += 1;
        return Ok(());
    }

    // Existing file: drift check
    let existing_sha = sha256_file(dest_path);

    if registry_sha == existing_sha {
        println!(
            "  {}",
            format!(
                "IN SYNC:   {} (sha: {}...)",
                dest_path.display(),
                &registry_sha[..12]
            )
            .green()
        );
        return Ok(());
    }

    // Drift found
    result.drifts_found += 1;

    if args.mode == Mode::Conformance {
        println!(
            "  {}",
            format!("DRIFT:     {}", dest_path.display())
                .yellow()
                .bold()
        );
        println!("    registry: {}...", &registry_sha[..16]);
        println!("    deployed: {}...", &existing_sha[..16]);
        return Ok(());
    }

    // Upgrade mode: compare versions
    if args.upgrade {
        let reg_ver = extract_version(source_path);
        let deploy_ver = extract_version(dest_path);
        let should_deploy = if reg_ver != "0.0.0" && deploy_ver != "0.0.0" {
            semver_gt(&reg_ver, &deploy_ver)
        } else {
            match skill.update_policy.as_str() {
                "always" => true,
                "newer" => mtime_newer(source_path, dest_path),
                _ => false,
            }
        };

        if !should_deploy {
            println!(
                "  {}",
                format!(
                    "SKIP:      {} (registry v{} <= deployed v{})",
                    dest_path.display(),
                    reg_ver,
                    deploy_ver
                )
                .yellow()
            );
            return Ok(());
        }

        // Find next unused version number
        let candidate = find_next_version(dest_path, skill.dest_dir == ".claude/rules");

        if args.dry_run {
            println!(
                "  {}",
                format!("UPGRADE:   would create {}", candidate.display()).green()
            );
            println!(
                "             (registry v{} > deployed v{})",
                reg_ver, deploy_ver
            );
        } else {
            fs::copy(source_path, &candidate)
                .with_context(|| format!("Failed to write upgrade to {}", candidate.display()))?;
            println!(
                "  {}",
                format!(
                    "UPGRADED:  {} (v{} over v{})",
                    candidate.display(),
                    reg_ver,
                    deploy_ver
                )
                .green()
            );
        }
        result.files_upgraded += 1;
        return Ok(());
    }

    // Default mode with drift but no --upgrade: just report
    println!(
        "  {}",
        format!("DRIFT:     {}", dest_path.display()).yellow()
    );
    println!("    registry: {}...", &registry_sha[..16]);
    println!("    deployed: {}...", &existing_sha[..16]);
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn sha256_file(path: &Path) -> String {
    match std::process::Command::new("sha256sum").arg(path).output() {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .trim()
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string(),
        _ => "sha256-fallback".to_string(),
    }
}

fn extract_version(path: &Path) -> String {
    let content = fs::read_to_string(path).unwrap_or_default();
    for line in content.lines() {
        if let Some(ver) = line.trim().strip_prefix('#').map(|s| s.trim()) {
            if let Some(Some(v)) = humantime_parse_version(ver) {
                return v.to_string();
            }
        }
        // Also try without # prefix (YAML frontmatter style)
        if let Some(Some(v)) = humantime_parse_version(line.trim()) {
            return v.to_string();
        }
    }
    "0.0.0".to_string()
}

/// Try to match `# Version: X.Y.Z` or `Version: X.Y.Z` (frontmatter)
fn humantime_parse_version(s: &str) -> Option<Option<String>> {
    let s = s.trim_start_matches(|c: char| c.is_whitespace());
    if let Some(stripped) = s.strip_prefix("Version:") {
        let trimmed = stripped.trim();
        if trimmed.len() >= 5 && trimmed.chars().filter(|&c| c == '.').count() >= 2 {
            return Some(Some(trimmed.to_string()));
        }
    }
    None
}

fn semver_gt(a: &str, b: &str) -> bool {
    let parse = |s: &str| -> (u32, u32, u32) {
        let mut parts = s.split('.');
        let maj = parts
            .next()
            .and_then(|p| p.parse::<u32>().ok())
            .unwrap_or(0);
        let min = parts
            .next()
            .and_then(|p| p.parse::<u32>().ok())
            .unwrap_or(0);
        let pat = parts
            .next()
            .and_then(|p| p.parse::<u32>().ok())
            .unwrap_or(0);
        (maj, min, pat)
    };
    let (a1, a2, a3) = parse(a);
    let (b1, b2, b3) = parse(b);
    if a1 > b1 {
        return true;
    }
    if a1 < b1 {
        return false;
    }
    if a2 > b2 {
        return true;
    }
    if a2 < b2 {
        return false;
    }
    a3 > b3
}

fn find_next_version(dest_path: &Path, is_rule: bool) -> PathBuf {
    let parent = dest_path.parent().unwrap();
    let base_name = dest_path.file_stem().unwrap().to_string_lossy().to_string();
    let ext = dest_path.extension().and_then(|e| e.to_str()).unwrap_or("");

    let mut counter = 1;
    loop {
        let name = if is_rule {
            format!("{}.v{}.{}", base_name, counter, ext)
        } else {
            // For skills dir: create <name>.v2/ alongside with SKILL.md inside
            return parent.join(format!("{}.v{}", &base_name, counter));
        };

        let candidate = if is_rule {
            parent.join(&name)
        } else {
            parent.join(name).join("SKILL.md")
        };

        if !candidate.exists() {
            return candidate;
        }
        counter += 1;
    }
}

/// Load projects from .meta.yaml using serde_yaml
fn load_projects(yaml_path: &Path) -> Result<Vec<ProjectEntry>> {
    let content = fs::read_to_string(yaml_path)
        .with_context(|| format!("Failed to read {}", yaml_path.display()))?;

    // Parse top-level structure: look for "projects:" key
    let parsed: serde_yaml::Value =
        serde_yaml::from_str(&content).context("YAML parse error in .meta.yaml")?;

    let projects_val = parsed
        .get("projects")
        .and_then(|v| v.as_mapping())
        .context(".meta.yaml missing 'projects' key")?;

    let mut entries = Vec::new();
    for (name_val, cfg_val) in projects_val {
        let name = name_val.as_str().unwrap_or("").to_string();
        if name.is_empty() {
            continue;
        }

        let table = match cfg_val.as_mapping() {
            Some(m) => m,
            None => continue, // bare alias (e.g. `defaults:` or inline)
        };

        let repo = table
            .get("repo")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if repo.is_empty() {
            continue; // not a project entry (defaults:, etc.)
        }

        let tags: Vec<String> = match table.get("tags") {
            Some(serde_yaml::Value::Sequence(seq)) => seq
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect(),
            _ => vec![],
        };

        let path = table
            .get("path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        entries.push(ProjectEntry {
            name,
            repo,
            tags,
            path,
        });
    }

    Ok(entries)
}

fn build_tags_map(projects: &[ProjectEntry]) -> HashMap<String, Vec<String>> {
    let mut map = HashMap::new();
    for p in projects {
        map.insert(p.name.clone(), p.tags.clone());
    }
    map
}

fn select_targets(
    mode: &Mode,
    meta_root: &Path,
    tags_map: &HashMap<String, Vec<String>>,
    _projects: &[ProjectEntry],
) -> Result<Vec<String>> {
    let mut targets: Vec<String> = vec![];

    match mode {
        Mode::Pilot => {
            // Default: handoff + ruflo explicitly added even if not canon-tagged
            for default_name in &["handoff", "ruflo"] {
                let tb = meta_root.join(default_name);
                if tb.join(".claude").is_dir() && !targets.contains(&default_name.to_string()) {
                    targets.push(default_name.to_string());
                }
            }
            // Expand: first canon-tagged repos with .claude/ (up to 4 total)
            for (name, tags) in tags_map {
                if !tags.contains(&"canon".to_string()) {
                    continue;
                }
                let tb = meta_root.join(name);
                if tb.join(".claude").is_dir() && !targets.contains(&name.to_string()) {
                    targets.push(name.clone());
                }
                if targets.len() >= 4 {
                    break;
                }
            }
            // Expand: first ai-tagged repos (prefer those with most existing skills)
            let mut scored_ai: Vec<(usize, String)> = tags_map
                .iter()
                .filter(|(_, tags)| tags.contains(&"ai".to_string()))
                .map(|(name, _)| {
                    let tb = meta_root.join(name);
                    let sc = if tb.join(".claude/skills").is_dir() {
                        fs::read_dir(tb.join(".claude/skills"))
                            .ok()
                            .map(|r| r.filter_map(|e| e.ok()).count())
                            .unwrap_or(0)
                    } else {
                        0
                    };
                    (sc, name.clone())
                })
                .collect();
            scored_ai.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
            for (_, name) in scored_ai {
                if !targets.contains(&name) {
                    targets.push(name);
                }
                if targets.len() >= 4 {
                    break;
                }
            }
        }
        Mode::Expand => {
            for (name, tags) in tags_map {
                if tags.contains(&"ai".to_string())
                    || tags.contains(&"orchestration".to_string())
                    || tags.contains(&"canon".to_string())
                {
                    targets.push(name.clone());
                }
            }
        }
        Mode::All => {
            for (name, _) in tags_map {
                let tb = meta_root.join(name);
                if tb.join(".claude").is_dir() {
                    targets.push(name.clone());
                }
            }
        }
        Mode::Conformance => {
            for (name, _) in tags_map {
                let tb = meta_root.join(name);
                if tb.join(".claude").is_dir() {
                    targets.push(name.clone());
                }
            }
        }
    }

    targets.sort();
    targets.dedup();
    Ok(targets)
}

fn compute_target_base(meta_root: &Path, project: &ProjectEntry) -> PathBuf {
    if let Some(ref path) = project.path {
        if !path.is_empty() && *path != project.name {
            return meta_root.join(path);
        }
    }
    meta_root.join(&project.name)
}

// ─────────────────────────────────────────────────────────────────────────────
// Output
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct SyncResult {
    mode: String,
    dry_run: bool,
    upgrade_requested: bool,
    repos_scanned: usize,
    repos_skipped: usize,
    skills_evaluated: usize,
    files_created: usize,
    files_upgraded: usize,
    drifts_found: usize,
    errors: Vec<String>,
}

impl Default for SyncResult {
    fn default() -> Self {
        SyncResult {
            mode: "pilot".to_string(),
            dry_run: true,
            upgrade_requested: false,
            repos_scanned: 0,
            repos_skipped: 0,
            skills_evaluated: 0,
            files_created: 0,
            files_upgraded: 0,
            drifts_found: 0,
            errors: vec![],
        }
    }
}

fn output_summary(result: &SyncResult, mode: Mode, dry_run: bool, upgrade: bool) {
    let mode_str = match mode {
        Mode::Pilot => "PILOT",
        Mode::Expand => "EXPAND",
        Mode::All => "ALL",
        Mode::Conformance => "CONFORMANCE",
    };
    println!();
    println!("==============================================");
    println!(" Skill Distribution Report");
    println!("==============================================");
    println!(" Mode:         {}", mode_str);
    println!(" Dry-run:      {}", dry_run);
    println!(" Upgrade:      {}", upgrade);
    println!(" Conformance:  {}", mode == Mode::Conformance);
    println!(
        " Repos:        {} scanned, {} skipped",
        result.repos_scanned, result.repos_skipped
    );
    println!(" Skills eval:  {}", result.skills_evaluated);
    println!(
        " Files:        {} created, {} upgraded",
        result.files_created, result.files_upgraded
    );
    println!(" Drifts:       {}", result.drifts_found);
    if !result.errors.is_empty() {
        println!(" Errors:");
        for err in &result.errors {
            println!("  - {}", err);
        }
    }
    println!("==============================================");
}
