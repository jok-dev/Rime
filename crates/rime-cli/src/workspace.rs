//! What `rime check` and `rime lsp` both do before checking anything: find
//! rime.toml, canonicalize the roots, walk them, discover the Rojo project,
//! read every project file. One implementation so the daemon sees exactly
//! the file set the CLI sees.

use crate::config::Config;
use crate::discover::Discovery;
use rayon::prelude::*;
use rime_check::session::Session;
use rime_check::timings::Timings;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub struct Workspace {
    pub config: Config,
    pub std: rime_std::StdNames,
    /// Some in project mode.
    pub resolver: Option<rime_roblox::RojoResolver>,
    /// Which project file that is, or why there is none.
    pub project: ProjectChoice,
    /// Everything to check: mount files first (sorted, deduped), then
    /// requested extras outside any mount, in walk order.
    pub files: Vec<(PathBuf, String)>,
    /// Files under the roots — the ones the run was actually asked to check.
    pub requested: HashSet<PathBuf>,
    /// Roots under which the walk found no source file to check.
    /// `rime check` warns about each: a run that checks nothing passes.
    pub empty_roots: Vec<EmptyRoot>,
}

pub struct EmptyRoot {
    /// The root as typed.
    pub path: PathBuf,
    /// How many git-ignored entries the walk left out under it. Non-zero
    /// means the root is not empty, only ignored.
    pub git_ignored: usize,
}

/// Who picked the project file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectSource {
    /// `--project <file>`.
    Flag,
    /// `project = "<file>"` in rime.toml.
    Config,
    /// Found by walking up from the checked path.
    Found,
}

/// The Rojo project file a run reads, or why it reads none. Without one
/// there is no cross-file checking, and that is never silent: `line` is
/// what the run says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectChoice {
    Using { file: PathBuf, source: ProjectSource },
    /// `why` finishes the sentence "cross-file checking is off: ".
    Off { why: String },
}

/// How to name a project file by hand, for the messages that suggest it.
const HOW_TO_NAME_ONE: &str = "name one with --project <file> or `project = \"<file>\"` in rime.toml";

impl ProjectChoice {
    pub fn file(&self) -> Option<&Path> {
        match self {
            ProjectChoice::Using { file, .. } => Some(file),
            ProjectChoice::Off { .. } => None,
        }
    }

    /// One line saying which project file is in use, or that cross-file
    /// checking is off and why. `show` renders a path for the reader.
    pub fn line(&self, show: impl Fn(&Path) -> String) -> String {
        match self {
            ProjectChoice::Using { file, source } => {
                let from = match source {
                    ProjectSource::Flag => " (from --project)",
                    ProjectSource::Config => " (from `project` in rime.toml)",
                    ProjectSource::Found => "",
                };
                format!("using project file {}{from}", show(file))
            }
            ProjectChoice::Off { why } => format!("cross-file checking is off: {why}"),
        }
    }
}

/// A path for a message: relative to `base` when under it, without the
/// Windows `\\?\` verbatim prefix either way.
pub fn show_path(path: &Path, base: &Path) -> String {
    let shown = path.strip_prefix(base).unwrap_or(path);
    let s = shown.to_string_lossy();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
}

/// Pick and load the Rojo project file. In order: `flag` (`--project`,
/// relative to `cwd`), then rime.toml's `project`, then the nearest folder
/// at or above `first_root` that holds a project file
/// (`rime_roblox::find_project_file`).
///
/// A file named by hand that cannot be read is an error: the run was told
/// what to use. A file found by the search that cannot be read turns
/// cross-file checking off and says so.
fn choose_project(
    flag: Option<&Path>,
    config: &Config,
    cwd: &Path,
    first_root: Option<&Path>,
) -> Result<(Option<rime_roblox::RojoProject>, ProjectChoice), String> {
    let canon_cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let show = |p: &Path| show_path(p.strip_prefix(cwd).unwrap_or(p), &canon_cwd);

    let named = match (flag, config.project_file()) {
        (Some(file), _) => Some((cwd.join(file), ProjectSource::Flag, format!("--project {}", file.display()))),
        (None, Some(file)) => {
            Some((file.to_path_buf(), ProjectSource::Config, format!("`project` in rime.toml ({})", show(file))))
        }
        (None, None) => None,
    };
    if let Some((file, source, said)) = named {
        // Rojo takes a folder too, and reads its default project file.
        let file = if file.is_dir() { file.join(rime_roblox::DEFAULT_PROJECT_FILE) } else { file };
        // Canonical, like the roots: mount paths are built from this
        // file's folder and compared against walked paths as prefixes.
        let file = file.canonicalize().map_err(|e| format!("{said}: could not read the project file: {e}"))?;
        let project = rime_roblox::RojoProject::load_file(&file)
            .map_err(|e| format!("{said}: could not read the project file: {e}"))?;
        return Ok((Some(project), ProjectChoice::Using { file, source }));
    }

    let start = first_root.and_then(|p| if p.is_dir() { Some(p) } else { p.parent() });
    let Some(start) = start else {
        return Ok((None, ProjectChoice::Off { why: format!("no path to look for a Rojo project file from; {HOW_TO_NAME_ONE}") }));
    };
    match rime_roblox::find_project_file(start) {
        rime_roblox::ProjectSearch::Found(file) => match rime_roblox::RojoProject::load_file(&file) {
            Ok(project) => Ok((Some(project), ProjectChoice::Using { file, source: ProjectSource::Found })),
            Err(e) => Ok((None, ProjectChoice::Off { why: format!("could not read {}: {e}", show(&file)) })),
        },
        rime_roblox::ProjectSearch::Several { dir, files } => Ok((
            None,
            ProjectChoice::Off {
                why: format!(
                    "`{}` has {} project files ({}) and no {}, so none was picked; {HOW_TO_NAME_ONE}",
                    show_dir(&dir, &canon_cwd),
                    files.len(),
                    files.join(", "),
                    rime_roblox::DEFAULT_PROJECT_FILE,
                ),
            },
        )),
        rime_roblox::ProjectSearch::None => Ok((
            None,
            ProjectChoice::Off {
                why: format!(
                    "no Rojo project file (*.project.json) at or above `{}`; {HOW_TO_NAME_ONE}",
                    show_dir(start, &canon_cwd),
                ),
            },
        )),
    }
}

/// `show_path` for a folder: the base itself is `.`, not an empty string.
fn show_dir(dir: &Path, base: &Path) -> String {
    let shown = show_path(dir, base);
    if shown.is_empty() { ".".to_string() } else { shown }
}

impl Workspace {
    /// `roots` as typed; `config_dir` is where rime.toml discovery starts
    /// (the CLI's cwd; the daemon's workspace folder). `project_file` is
    /// `--project`, relative to `config_dir`. `gitignore`: leave out what
    /// git ignores under the roots (see `discover`).
    pub fn load(
        roots: &[PathBuf],
        config_path: Option<&Path>,
        project_file: Option<&Path>,
        config_dir: &Path,
        gitignore: bool,
        mut timings: Option<&mut Timings>,
    ) -> Result<Workspace, String> {
        let config = Config::discover(config_path, config_dir)?;
        let std = config.build_std_names()?;

        // One canonicalize per ROOT, never per file: everything walked
        // beneath a canonical root is already in canonical form
        // (rime_check::walk), so the requested set, the project's mount
        // files, and rime.toml's ignore prefixes all compare as plain
        // prefixes. A root that does not exist stays as typed and walks to
        // nothing.
        let typed_roots = roots;
        let roots: Vec<PathBuf> =
            roots.iter().map(|p| p.canonicalize().unwrap_or_else(|_| p.clone())).collect();

        // Project mode: a Rojo project file (named, or found at or above
        // the first root) enables cross-file call conformance. Interfaces
        // are extracted from the WHOLE project so requires resolve even
        // when only a subtree is checked; reports stay limited to the
        // requested files. Loaded before the walk: the walk leaves out what
        // git ignores, and has to know the mounts to keep listing them.
        let load_start = std::time::Instant::now();
        let (project, choice) =
            choose_project(project_file, &config, config_dir, roots.first().map(PathBuf::as_path))?;
        if let Some(t) = timings.as_mut() {
            t.record("load rojo project", load_start.elapsed());
        }

        let walk_start = std::time::Instant::now();
        let mounts: Vec<PathBuf> =
            project.iter().flat_map(|p| p.mount_bases()).map(Path::to_path_buf).collect();
        let discovery = Discovery::new(gitignore, mounts);
        // `walked` is what the run is about. `listed` is that plus the
        // git-ignored files under a mount: the listing an unfiltered walk
        // gives each mount, which is what `seed_walk` needs.
        let mut walked: Vec<PathBuf> = Vec::new();
        let mut listed: Vec<PathBuf> = Vec::new();
        let mut empty_roots: Vec<EmptyRoot> = Vec::new();
        for (typed, root) in typed_roots.iter().zip(&roots) {
            let found = discovery.sources(root);
            if found.files.is_empty() {
                empty_roots.push(EmptyRoot { path: typed.clone(), git_ignored: found.git_ignored });
            }
            walked.extend(found.files);
            listed.extend(found.with_mounts);
        }
        if let Some(t) = timings.as_mut() {
            t.record("walk paths", walk_start.elapsed());
        }
        let requested: HashSet<PathBuf> = walked.iter().cloned().collect();

        let (files, resolver) = match project {
            Some(project) => {
                // Mounts inside a root reuse the walk above; the rest walk
                // here.
                let walk_start = std::time::Instant::now();
                project.seed_walk(&roots, &listed);
                let mount_files = project.mount_files();
                if let Some(t) = timings.as_mut() {
                    t.record("read files: walk mounts", walk_start.elapsed());
                }
                // From here `read files` is the reads alone.
                let read_start = std::time::Instant::now();
                // Reads fan out across threads: per-file open latency, not
                // bandwidth, is the cost (Windows especially). Order
                // preserved.
                let read: Vec<Option<(PathBuf, String)>> = mount_files
                    .into_par_iter()
                    .map(|path| std::fs::read_to_string(&path).ok().map(|src| (path, src)))
                    .collect();
                let mut have: HashSet<PathBuf> = HashSet::new();
                let mut files: Vec<(PathBuf, String)> = Vec::new();
                for (path, src) in read.into_iter().flatten() {
                    have.insert(path.clone());
                    files.push((path, src));
                }
                // Files requested but outside any mount still get checked,
                // in walk order after the sorted mount files.
                for path in &walked {
                    if have.insert(path.clone()) {
                        if let Ok(src) = std::fs::read_to_string(path) {
                            files.push((path.clone(), src));
                        }
                    }
                }
                if let Some(t) = timings.as_mut() {
                    t.record("read files", read_start.elapsed());
                }
                let resolver = rime_roblox::RojoResolver::new(project).with_trusted(config.trusted_requires());
                (files, Some(resolver))
            }
            None => {
                let read_start = std::time::Instant::now();
                let mut files = Vec::new();
                for path in &walked {
                    if config.ignored(path) {
                        continue;
                    }
                    match std::fs::read_to_string(path) {
                        Ok(src) => files.push((path.clone(), src)),
                        Err(_) => eprintln!("warning: could not read {}", path.display()),
                    }
                }
                if let Some(t) = timings.as_mut() {
                    t.record("read files", read_start.elapsed());
                }
                (files, None)
            }
        };
        Ok(Workspace { config, std, resolver, project: choice, files, requested, empty_roots })
    }

    /// Build the warm session. The resolver moves into it.
    pub fn session(self, timings: Option<&mut Timings>) -> (Session<'static>, Config, HashSet<PathBuf>) {
        let resolver: Option<Box<dyn rime_check::project::Resolver>> =
            self.resolver.map(|r| Box::new(r) as Box<dyn rime_check::project::Resolver>);
        let options = self.config.rule_options();
        let session = Session::with_options(self.files, resolver, self.std, options, timings);
        (session, self.config, self.requested)
    }
}
