use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
};

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::{
    config::{Ctx, ProfileDesc},
    entry::{Convenience, Entry, RelativePath, STUB},
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
                    let p = shellexpand::tilde_with_context(p, || {
                        Some(ctx.canon_home_dir.to_string_lossy())
                    })
                    .to_string();
                    let module = Module::new(e.name.to_owned(), PathBuf::from(p).canonicalize()?)?;
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

        for (name, present) in active
            .modules
            .iter()
            .map(|name| (name, modules.contains_key(name)))
        {
            if !present {
                return Err(anyhow!("active module '{}' not found", name));
            }
        }
        for (name, present) in required
            .modules
            .iter()
            .map(|name| (name, modules.contains_key(name)))
        {
            if !present {
                return Err(anyhow!("required module '{}' not found", name));
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

                self.sync_entry(&e, force, ctx)?;
            }
        }

        let prof = toml::to_string_pretty(&self.required_conf)?;
        fs::write(&ctx.profile_file, prof)?;
        Ok(())
    }

    fn sync_entry(&self, e: &Entry, force: bool, ctx: &Ctx) -> Result<()> {
        let privilege = e.get_priv(ctx)?;
        fs::create_dir_all(e.src.parent().unwrap())?;
        drop(privilege);

        if !e.src.exists() {
            println!(
                "creating symlink\n  src: {:?}\n  dst: {:?}",
                &e.src, &e.dest
            );
            e.symlink_to_src(ctx)?;
            return Ok(());
        }

        if e.src.canonicalize()? == e.dest {
            return Ok(());
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

    pub fn add(&mut self, src: impl AsRef<str>, ctx: &Ctx, dest: impl AsRef<str>) -> Result<()> {
        let src = src.as_ref();
        let dest = dest.as_ref();
        let Some(pos) = self.active_conf.modules.iter().position(|n| n == dest) else {
            return Err(anyhow!("module {} is not active", dest));
        };
        let dest_module = self.modules.get(dest).expect("checked above");

        let e = match dest_module.entry_from_src(dest_module.resolve_path(src, ctx)?, ctx) {
            Ok(e) => e,
            Err(PathResolutionError::InRepo) => {
                println!("the path {} is already in the repo.", src);
                return Ok(());
            }
            Err(PathResolutionError::OutsideRepo) => unreachable!(),
        };

        // give error when trying to add something to a module but other module already has the thing (only if other has higher precedence)
        for module in self.active_conf.modules[pos + 1..]
            .iter()
            .map(|name| self.modules.get(name).expect("checked in Profile::new"))
        {
            if module.home_entries.contains(e.relative.path())
                || module.non_home_entries.contains(e.relative.path())
            {
                return Err(anyhow!(
                    "path '{}' is already in module '{}' which has higher precedence than destination module '{}'",
                    src,
                    &module.name,
                    dest,
                ));
            }
        }

        let mut p = dest_module.module_dir.clone();
        for c in e.relative.clone().relative().parent().unwrap().components() {
            let std::path::Component::Normal(c) = c else {
                unreachable!()
            };
            p.push(format!(".{}.{STUB}", c.to_str().unwrap()));

            if p.exists() {
                return Err(anyhow!(
                    "path is already in a directory managed by configma\n  src: {}\n  dir: {:?}\n",
                    src,
                    p,
                ));
            }
            p.pop();
            p.push(c);
        }

        if dest_module.contains(&e) {
            println!("path is already maintained by configma: {}\n", src);
            return Ok(());
        }

        println!("moving path\n  src: {:?}\n  dst: {:?}\n", &e.src, &e.dest);

        fs::create_dir_all(e.dest.parent().unwrap())?;

        if e.src.is_file() {
            // the files should have read permissions without root
            fs::copy(&e.src, &e.dest)?;
            e.rm_src_file(ctx)?;
        } else if e.src.is_dir() {
            // the files should have read permissions without root
            fs_extra::dir::copy(
                &e.src,
                &e.dest,
                &fs_extra::dir::CopyOptions::new()
                    .copy_inside(false)
                    .content_only(true),
            )?;
            e.rm_src_dir_all(ctx)?;
            let _ = fs::File::create(
                e.dest
                    .parent()
                    .expect("path cannot be root")
                    .join(format!(".{}.{STUB}", e.dest.name())),
            )?;
        } else {
            return Err(anyhow!(
                "cannot handle this type of file or whatever: {}",
                &src
            ));
        }

        e.symlink_to_src(ctx)?;

        let dest_module = self.modules.get_mut(dest).expect("checked above");
        match &e.relative {
            RelativePath::Home(p) => dest_module.home_entries.insert(p.clone()),
            RelativePath::NonHome(p) => dest_module.non_home_entries.insert(p.clone()),
        };
        Ok(())
    }

    pub fn remove_from_active(&mut self, src: impl AsRef<str>, ctx: &Ctx) -> Result<()> {
        let src = src.as_ref();
        let mut pos = None;
        for (i, m) in self
            .active_conf
            .modules
            .iter()
            .enumerate()
            .rev()
            .map(|(i, m)| (i, self.modules.get(m).expect("checked in Profile::new")))
        {
            let e = m.entry(src, ctx)?;
            if m.contains(&e) {
                pos = Some(i);
                break;
            }
        }
        let Some(pos) = pos else {
            return Err(anyhow!("no active module contains '{}'", src));
        };
        let module = self
            .modules
            .get(&self.active_conf.modules[pos])
            .expect("checked above");

        let e = module.entry(src, ctx)?;
        self._remove(&e, ctx, module)?;

        let module = self
            .modules
            .get_mut(&self.active_conf.modules[pos])
            .expect("checked above");
        match &e.relative {
            RelativePath::Home(p) => module.home_entries.remove(p),
            RelativePath::NonHome(p) => module.non_home_entries.remove(p),
        };

        self.sync_active(&e, ctx)?;
        Ok(())
    }

    // find module using whatever user picked
    // move file from module repo to dump
    // delete entry from module in memory (just for consistency)
    // check if any other module has the same entry
    // either simlink the other module's entry, or restore entry from dump to the required location
    pub fn remove(&mut self, src: impl AsRef<str>, ctx: &Ctx, name: impl AsRef<str>) -> Result<()> {
        let src = src.as_ref();
        let name = name.as_ref();
        let Some(_) = self.active_conf.modules.iter().position(|n| n == name) else {
            return Err(anyhow!("module '{}' is not active", name));
        };
        let module = self.modules.get(name).expect("checked above");

        let e = module.entry(src, ctx)?;
        self._remove(&e, ctx, module)?;

        let module = self.modules.get_mut(name).expect("checked above");
        match &e.relative {
            RelativePath::Home(p) => module.home_entries.remove(p),
            RelativePath::NonHome(p) => module.non_home_entries.remove(p),
        };

        self.sync_active(&e, ctx)?;
        Ok(())
    }

    fn sync_active(&self, e: &Entry, ctx: &Ctx) -> Result<()> {
        for m in self
            .active_conf
            .modules
            .iter()
            .rev()
            .map(|m| self.modules.get(m).expect("checked in Profile::new"))
        {
            if m.contains(e) {
                self.sync_entry(e, true, ctx)?;
                return Ok(());
            }
        }
        Ok(())
    }

    fn _remove(&self, e: &Entry, ctx: &Ctx, module: &Module) -> Result<()> {
        if module.contains(e) {
            println!("restoring path\n  src: {:?}\n  dst: {:?}\n", e.src, e.dest,);

            e.rm_src_file(ctx)?;
            if e.dest.is_dir() {
                fs::remove_file(
                    e.dest
                        .parent()
                        .expect("path cannot be root")
                        .join(format!(".{}.{STUB}", e.dest.name())),
                )?;
                e.copy_dir_to_src(ctx)?;
                fs::remove_dir_all(&e.dest)?;
            } else if e.dest.is_file() {
                e.copy_file_to_src(ctx)?;
                fs::remove_file(&e.dest)?;
            } else {
                return Err(anyhow!(
                    "cannot handle this type of file or whatever: '{:?}'",
                    &e.src
                ));
            }

            // remove empty parent dirs
            let mut parent = e.relative.clone().relative();
            while parent.pop() && !parent.to_string_lossy().is_empty() {
                let p = module.module_dir.join(&parent);
                if p.read_dir()?.count() == 0 {
                    fs::remove_dir(p)?;
                } else {
                    break;
                }
            }
        } else {
            return Err(anyhow!(
                "file '{:?}' not in module '{}'",
                &e.src,
                module.name
            ));
        }
        Ok(())
    }
}
