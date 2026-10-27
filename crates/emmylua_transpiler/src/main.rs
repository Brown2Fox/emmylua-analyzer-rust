use std::{fs, path::PathBuf, process::ExitCode, sync::Arc};

use clap::Parser;
use emmylua_code_analysis::{
    CheckConfig, EmmyLuaAnalysis, Emmyrc, EmmyrcLuaVersion, WorkspaceFolder,
    build_workspace_folders, collect_workspace_files, file_path_to_uri, load_configs,
    uri_to_file_path,
};
use emmylua_parser::LuaLanguageLevel;
use emmylua_transpiler::transpile;
use lsp_types::DiagnosticSeverity;

#[derive(Parser)]
#[command(version, about = "Experimental typed Lua to Lua compiler")]
struct Args {
    /// Typed Lua source file (conventionally .cua).
    input: PathBuf,
    /// Destination Lua file. Written only when checking succeeds.
    #[arg(
        short,
        long,
        required_unless_present = "check",
        conflicts_with = "check"
    )]
    output: Option<PathBuf>,
    /// Check without emitting a file.
    #[arg(long)]
    check: bool,
    /// EmmyLua library file or directory; may be repeated. Libraries are indexed, not diagnosed.
    #[arg(short = 'L', long)]
    library: Vec<PathBuf>,
    /// Load project modules, libraries and .emmyrc configuration from this root.
    #[arg(long)]
    workspace: Option<PathBuf>,
    /// Target Lua version.
    #[arg(long, value_parser = ["5.1", "5.2", "5.3", "5.4", "5.5", "luajit"])]
    lua_version: Option<String>,
}

fn main() -> ExitCode {
    match run(Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<(), String> {
    let input = args.input.canonicalize().map_err(|e| e.to_string())?;
    if args
        .output
        .as_ref()
        .and_then(|p| p.canonicalize().ok())
        .as_ref()
        == Some(&input)
    {
        return Err("output must differ from the source file".into());
    }
    let source = fs::read_to_string(&input).map_err(|e| e.to_string())?;
    let input = normalize_path(input);
    let workspace = args
        .workspace
        .as_ref()
        .map(|path| path.canonicalize().map(normalize_path))
        .transpose()
        .map_err(|e| e.to_string())?;
    let mut config = if let Some(root) = &workspace {
        let paths = [".emmyrc.json", ".emmyrc.lua"]
            .into_iter()
            .map(|name| root.join(name))
            .filter(|p| p.exists())
            .collect();
        let mut config = load_configs(paths, None);
        config.pre_process_emmyrc(root);
        config
    } else {
        let mut config = Emmyrc::default();
        config.strict.no_implicit_any = true;
        config.strict.check_unknown = true;
        config
    };
    let (mut level, version) = match args.lua_version.as_deref().unwrap_or("5.1") {
        "5.2" => (LuaLanguageLevel::Lua52, EmmyrcLuaVersion::Lua52),
        "5.3" => (LuaLanguageLevel::Lua53, EmmyrcLuaVersion::Lua53),
        "5.4" => (LuaLanguageLevel::Lua54, EmmyrcLuaVersion::Lua54),
        "5.5" => (LuaLanguageLevel::Lua55, EmmyrcLuaVersion::Lua55),
        "luajit" => (LuaLanguageLevel::LuaJIT, EmmyrcLuaVersion::LuaJIT),
        _ => (LuaLanguageLevel::Lua51, EmmyrcLuaVersion::Lua51),
    };
    if workspace.is_none() || args.lua_version.is_some() {
        config.runtime.version = version;
    } else {
        level = config.runtime.version.get_language_level();
    }
    let output = transpile(&source, level).map_err(|error| {
        let (line, column) = location(&source, error.offset);
        format!("{}:{line}:{column}: {}", input.display(), error.message)
    })?;
    let config = Arc::new(config);
    let mut analysis = EmmyLuaAnalysis::new();
    analysis.update_config(config.clone());
    analysis.add_main_workspace(
        input
            .parent()
            .ok_or("source has no parent directory")?
            .to_path_buf(),
    );
    analysis.init_std_lib(None);
    if let Some(root) = workspace {
        if !input.starts_with(&root) {
            return Err("source must be inside --workspace".into());
        }
        let folders = build_workspace_folders(&[WorkspaceFolder::new(root, false)], &config);
        for folder in &folders {
            if folder.is_library {
                analysis.add_library_workspace(folder);
            } else {
                analysis.add_main_workspace(folder.root.clone());
            }
        }
        let files = collect_workspace_files(&folders, &config, None, None)
            .into_iter()
            .map(|file| file.into_tuple())
            .collect();
        analysis.update_files_by_path(files);
    }
    let mut libraries = vec![];
    for path in args.library {
        let path = path
            .canonicalize()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let path = normalize_path(path);
        if !input.starts_with(&path) {
            analysis.add_library_workspace(&WorkspaceFolder::new(path.clone(), true));
        }
        if path.is_file() {
            libraries.push(path);
        } else {
            for entry in walkdir::WalkDir::new(path).sort_by_file_name() {
                let entry = entry.map_err(|e| e.to_string())?;
                if entry.file_type().is_file()
                    && entry.path().extension().is_some_and(|ext| ext == "lua")
                {
                    libraries.push(entry.into_path());
                }
            }
        }
    }
    libraries.sort();
    libraries.dedup();
    let mut files = vec![];
    for path in libraries {
        if path == input {
            continue;
        }
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        files.push((path, Some(text)));
    }
    analysis.update_files_by_path(files);
    let id = analysis
        .update_file_by_path(&input.with_extension("cua"), Some(source.clone()))
        .ok_or("cannot register source file")?;
    let model = analysis.semantic_model(id);
    if model
        .workspace_id_of(id)
        .is_none_or(|workspace| !workspace.is_main())
    {
        return Err("source must belong to the main workspace".into());
    }
    if model.file_facts().is_some_and(|facts| facts.is_meta) {
        return Err(
            "definition-only (@meta) files cannot be compiled as executable sources".into(),
        );
    }
    let diagnostics = analysis
        .diagnose_file_with_config(id, Arc::new(CheckConfig::new(&config)))
        .ok_or("cannot check source file")?;
    let mut failures = 0;
    for diagnostic in diagnostics {
        if !matches!(
            diagnostic.severity,
            Some(DiagnosticSeverity::ERROR | DiagnosticSeverity::WARNING)
        ) {
            continue;
        }
        failures += 1;
        let original = offset_at(
            &source,
            diagnostic.range.start.line,
            diagnostic.range.start.character,
        );
        let (line, column) = location(&source, original);
        eprintln!(
            "{}:{line}:{column}: {}",
            input.display(),
            diagnostic.message
        );
    }
    if failures > 0 {
        return Err(format!("compilation stopped: {failures} diagnostic(s)"));
    }
    if let Some(path) = args.output {
        fs::write(&path, output.lua).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("{}", path.display());
    } else {
        println!("No issues found.");
    }
    Ok(())
}

fn location(text: &str, offset: usize) -> (usize, usize) {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let prefix = &text[..offset];
    (
        prefix.bytes().filter(|b| *b == b'\n').count() + 1,
        prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1,
    )
}

fn offset_at(text: &str, line: u32, column: u32) -> usize {
    let start = text
        .split_inclusive('\n')
        .take(line as usize)
        .map(str::len)
        .sum::<usize>();
    let mut utf16 = 0;
    for (offset, ch) in text[start..].char_indices() {
        if utf16 >= column || ch == '\n' {
            return start + offset;
        }
        utf16 += ch.len_utf16() as u32;
    }
    text.len()
}

fn normalize_path(path: PathBuf) -> PathBuf {
    file_path_to_uri(&path)
        .and_then(|uri| uri_to_file_path(&uri))
        .unwrap_or(path)
}
