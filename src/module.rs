use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::{
    config::Ctx,
    entry::{generate_entry_set, Convenience, Entry, RelativePath, HOME},
};

#[derive(Deserialize, Serialize, Debug)]
pub struct Module {
    pub name: String,
    pub module_dir: PathBuf,
    pub home_entries: HashSet<PathBuf>,
    pub non_home_entries: HashSet<PathBuf>,
}

pub enum PathResolutionError {
    InRepo,
    OutsideRepo,
}

impl Module {
    pub fn new(name: String, repo: impl AsRef<Path>) -> Result<Self> {
        let repo = repo.as_ref();
        if !repo.exists() {
            return Err(anyhow!("path does not exist: {:?}", repo));
        }
        let module_dir = repo.join(&name);

        if !module_dir.exists() {
            return Err(anyhow!("path does not exist: {:?}", module_dir));
        }

        let home = module_dir.join(HOME);
        if !home.exists() {
            fs::create_dir(&home)?;
        }

        let home_entries = generate_entry_set(home)?;

        let mut entries = HashSet::new();
        for dir in fs::read_dir(&module_dir)? {
            let dir = dir?;
            let path = dir.path();

            if path.name() == HOME {
                continue;
            }

            if path.is_file() {
                entries.insert(path.strip_prefix(&module_dir)?.to_path_buf());
            } else if path.is_dir() {
                let dir_entries = generate_entry_set(&path)?;
                entries.extend(
                    dir_entries
                        .into_iter()
                        .map(|p| PathBuf::from(path.file_name().expect("no file name")).join(p)),
                );
            } else {
                println!("ignoring unhandlable path: {:?}", &path);
            }
        }

        let s = Self {
            name,
            module_dir,
            home_entries,
            non_home_entries: entries,
        };
        Ok(s)
    }

    pub fn contains(&self, e: &Entry) -> bool {
        match &e.relative {
            RelativePath::Home(p) => self.home_entries.contains(p),
            RelativePath::NonHome(p) => self.non_home_entries.contains(p),
        }
    }

    pub fn unlink_all(&self, ignore_non_links: bool, ctx: &Ctx) -> Result<()> {
        for e in
            self.home_entries
                .iter()
                .map(|p| self.entry_from_relative(&RelativePath::Home(p.to_path_buf()), ctx))
                .chain(self.non_home_entries.iter().map(|p| {
                    self.entry_from_relative(&RelativePath::NonHome(p.to_path_buf()), ctx)
                }))
        {
            if !e.src.is_symlink() || e.src.canonicalize()? != e.dest {
                if ignore_non_links {
                    continue;
                } else {
                    return Err(anyhow!("bad Entry: {:?}.", &e.relative));
                }
            }

            println!("deleting symlink: {:?}\n", &e.src);
            e.rm_src_file(ctx)?;
        }

        Ok(())
    }

    pub fn resolve_path(&self, path: impl AsRef<str>, ctx: &Ctx) -> Result<PathBuf> {
        let filename = PathBuf::from(
            shellexpand::tilde_with_context(path.as_ref(), || {
                Some(ctx.canon_home_dir.to_string_lossy())
            })
            .into_owned(),
        );
        let src = filename
            .parent()
            .map(|p| {
                if p.to_string_lossy().is_empty() {
                    PathBuf::from("./").canonicalize()
                } else {
                    p.canonicalize()
                }
            })
            .unwrap()?
            .join(filename.file_name().unwrap());
        Ok(src)
    }

    pub fn entry_from_dest(
        &self,
        dest: impl AsRef<Path>,
        ctx: &Ctx,
    ) -> Result<Entry, PathResolutionError> {
        let dest = dest.as_ref();

        if !dest.starts_with(&ctx.canon_repo) {
            return Err(PathResolutionError::OutsideRepo);
        }

        let relative = dest.strip_prefix(&self.module_dir).unwrap();
        let (src, relative) = match relative.starts_with(HOME) {
            true => {
                let stripped = relative.strip_prefix(HOME).unwrap().to_path_buf();
                (
                    ctx.canon_home_dir.join(&stripped),
                    RelativePath::Home(stripped),
                )
            }
            false => (
                PathBuf::from("/").join(relative),
                RelativePath::NonHome(relative.to_path_buf()),
            ),
        };

        Ok(Entry {
            src,
            relative,
            dest: dest.to_path_buf(),
        })
    }

    pub fn entry_from_src(
        &self,
        src: impl AsRef<Path>,
        ctx: &Ctx,
    ) -> Result<Entry, PathResolutionError> {
        let src = src.as_ref();

        if src.starts_with(&ctx.canon_repo) {
            return Err(PathResolutionError::InRepo);
        }

        let (dest, relative) = match src.starts_with(&ctx.canon_home_dir) {
            true => {
                let stripped = src.strip_prefix(&ctx.canon_home_dir).unwrap();
                (
                    self.module_dir.join(HOME).join(stripped),
                    RelativePath::Home(stripped.to_path_buf()),
                )
            }
            false => {
                let stripped = src.strip_prefix("/").expect("path must be absolute");
                (
                    self.module_dir.join(stripped),
                    RelativePath::NonHome(stripped.to_path_buf()),
                )
            }
        };

        Ok(Entry {
            src: src.to_path_buf(),
            relative,
            dest,
        })
    }

    pub fn entry_from_relative(&self, rel: &RelativePath, ctx: &Ctx) -> Entry {
        match rel {
            RelativePath::Home(p) => Entry {
                src: ctx.canon_home_dir.join(p),
                relative: rel.clone(),
                dest: self.module_dir.join(HOME).join(p),
            },
            RelativePath::NonHome(p) => Entry {
                src: PathBuf::from("/").join(p),
                relative: rel.clone(),
                dest: self.module_dir.join(p),
            },
        }
    }

    pub fn entry(&self, path_str: impl AsRef<str>, ctx: &Ctx) -> Result<Entry> {
        let path = self.resolve_path(&path_str, ctx)?;
        match self.entry_from_dest(&path, ctx) {
            Ok(p) => Ok(p),
            Err(PathResolutionError::OutsideRepo) => match self.entry_from_src(&path, ctx) {
                Ok(p) => Ok(p),
                Err(_) => unreachable!(),
            },
            Err(_) => unreachable!(),
        }
    }
}
