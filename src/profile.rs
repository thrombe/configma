use std::{collections::{HashMap, HashSet}, fs, path::PathBuf};

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::{
    config::{Ctx, ProfileDesc},
    module::Module,
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

    pub fn sync(&self, force: bool, ctx: &Ctx) -> Result<()> {
        for name in self
            .active_conf
            .modules.iter().collect::<HashSet<_>>()
            .difference(&self.required_conf.modules.iter().collect::<HashSet<_>>())
        {
            let module = self.modules.get(name.as_str()).expect("checked earlier");
            module.unlink_all(force, ctx)?;
        }

        // TODO: don't try to sync stuff that is duplicated in multiple modules
        // TODO: give error when trying to add something to a module but other module already has the thing (only if other has higher precedence)
        let mut synced_home = HashSet::new();
        let mut synced_non_home = HashSet::new();
        for name in self.required_conf.modules.iter() {
            let module = self.modules.get(name).expect("checked earlier");
            module.sync(force, ctx)?;

            for e in module.home_entries.iter() {
                synced_home.insert(e);
            }
            for e in module.non_home_entries.iter() {
                synced_non_home.insert(e);
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
