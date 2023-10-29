use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    config::{Ctx, ProfileDesc},
    entry::{RelativePath, STUB},
    module::{Module, PathResolutionError},
};

#[derive(Deserialize, Serialize, Debug)]
pub struct Profile {
    pub modules: HashMap<String, Module>,

    pub active_conf: ProfileDesc,
    pub required_conf: ProfileDesc,
}

impl Profile {
    pub fn new(active: ProfileDesc, required: ProfileDesc, ctx: &Ctx) -> Result<Self> {
        // get modules.
        // any modules that are in the main repo
        // modules mentioned in the config (probably from some other source)
        let mut modules = HashMap::new();
        for e in fs::read_dir(&ctx.canon_repo)? {
            let e = e?;
            if !e.metadata()?.is_dir() {
                continue;
            }
            let name = e.file_name().into_string().expect("non utf name");
            let module = Module::new(name.to_owned(), &ctx.canon_repo)?;
            modules.insert(name.to_owned(), module);
        }

        for e in &ctx.conf.modules {
            match &e.path {
                Some(p) => {
                    let p = shellexpand::tilde_with_context(p, || Some(ctx.home_dir.to_string_lossy())).to_string();
                    let module = Module::new(e.name.to_owned(), p)?;
                    modules.insert(e.name.to_owned(), module);
                }
                None => {
                    if !modules.contains_key(&e.name) {
                        return Err(anyhow!(
                            "no module with name {} found in the repo.",
                            &e.name
                        ));
                    }
                }
            }
        }

        let s = Self {
            modules,
            active_conf: active,
            required_conf: required,
        };
        Ok(s)
    }

    /// creates new symlinks for any entry that does not have a symlink
    pub fn sync(&self, force: bool, ctx: &Ctx) -> Result<()> {
        for name in self
            .active_conf
            .modules
            .iter()
            .collect::<HashSet<_>>()
            .difference(&self.required_conf.modules.iter().collect::<HashSet<_>>())
        {
            let module = self.modules.get(name.as_str()).expect("checked earlier");
            module.unlink_all(force, ctx)?;
        }

        // TODO: give error when trying to add something to a module but other module already has the thing (only if other has higher precedence)
        let mut synced = HashSet::new();
        for name in self.required_conf.modules.iter().rev() {
            let module = self.modules.get(name).expect("checked in Profile::new");

            for e in module
                .home_entries
                .iter()
                .map(|p| module.entry_from_relative(&RelativePath::Home(p.to_path_buf()), ctx))
                .chain(module.non_home_entries.iter().map(|p| {
                    module.entry_from_relative(&RelativePath::NonHome(p.to_path_buf()), ctx)
                }))
            {
                let src = e.src.clone();
                // ignore if already synced by a module with higher precedence
                if synced.contains(&src) {
                    continue;
                }
                synced.insert(src);

                let privilege = e.get_priv(ctx)?;
                fs::create_dir_all(e.src.parent().unwrap())?;
                drop(privilege);

                if !e.src.exists() {
                    println!(
                        "creating symlink\n  src: {:?}\n  dst: {:?}",
                        &e.src, &e.dest
                    );
                    e.symlink_to_src(ctx)?;
                    continue;
                }

                if e.src.canonicalize()? == e.dest {
                    continue;
                }

                println!(
                    "creating symlink\n  src: {:?}\n  dst: {:?}",
                    &e.src, &e.dest
                );

                if !force {
                    return Err(anyhow!(
                        "there is already a file/dir at: {:?}. use -f flag to force sync",
                        &e.src
                    ));
                }

                let dump_to = ctx.dump_dir.join(e.relative.clone().relative());

                println!(
                    "moving contents to dump\n  src: {:?}\n  dump: {:?}",
                    &e.src, &dump_to
                );

                fs::create_dir_all(dump_to.parent().unwrap())?;

                if e.src.is_file() || e.src.is_symlink() {
                    let _ = fs::copy(&e.src, &dump_to)?;
                    e.rm_src_file(ctx)?;
                } else if e.src.is_dir() {
                    fs_extra::dir::copy(
                        &e.src,
                        &dump_to,
                        &fs_extra::dir::CopyOptions::new()
                            .copy_inside(false)
                            .content_only(true),
                    )?;
                    e.rm_src_dir_all(ctx)?;
                } else {
                    return Err(anyhow!(
                        "cannot handle this type of file or whatever: {:?}",
                        &e.src
                    ));
                }
                println!();

                e.symlink_to_src(ctx)?;
            }
        }

        let prof = toml::to_string_pretty(&self.required_conf)?;
        fs::write(&ctx.profile_file, prof)?;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        let home = PathBuf::from("home");
        let mut dirs = HashMap::new();
        for m in self.modules.values() {
            for p in &m.home_entries {
                if p.is_dir() {
                    dirs.insert(home.join(p), &m.name);
                }
            }
            for p in &m.non_home_entries {
                if p.is_dir() {
                    dirs.insert(p.clone(), &m.name);
                }
            }
        }

        for m in self.modules.values() {
            for p in &m.home_entries {
                let mut path = home.join(p);
                while path.pop() {
                    if dirs.contains_key(&path) {
                        return Err(anyhow!(
                            "path {:?} from module {} contains {:?} from module {}",
                            &path,
                            dirs.get(&path).expect("inserted earlier in same function"),
                            p,
                            &m.name
                        ));
                    }
                }
            }
            for p in &m.non_home_entries {
                let mut path = p.clone();
                while path.pop() {
                    if dirs.contains_key(&path) {
                        return Err(anyhow!(
                            "path {:?} from module {} contains {:?} from module {}",
                            &path,
                            dirs.get(&path).expect("inserted earlier in same function"),
                            p,
                            &m.name
                        ));
                    }
                }
            }
        }

        Ok(())
    }
}
