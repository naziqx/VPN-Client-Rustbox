//! Persistent profile/group storage (a single JSON file).

use std::path::{Path, PathBuf};

use anyhow::Context;
use rustbox_platform::Platform;
use serde::{Deserialize, Serialize};

use crate::model::{Group, GroupId, Latency, Profile, ProfileId, Subscription};

pub const DEFAULT_GROUP: GroupId = 0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Store {
    pub groups: Vec<Group>,
    pub profiles: Vec<Profile>,
    next_id: u64,
    #[serde(skip)]
    path: PathBuf,
}

impl Store {
    pub fn path(platform: &dyn Platform) -> PathBuf {
        platform.data_dir().join("profiles.json")
    }

    fn empty(path: PathBuf) -> Self {
        Self {
            groups: vec![Group {
                id: DEFAULT_GROUP,
                name: "Default".into(),
                subscription: None,
            }],
            profiles: Vec::new(),
            next_id: 1,
            path,
        }
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let mut store = match std::fs::read_to_string(path) {
            Ok(s) => serde_json::from_str::<Store>(&s)
                .with_context(|| format!("invalid {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::empty(path.to_path_buf()),
            Err(e) => return Err(e).with_context(|| format!("failed to read {}", path.display())),
        };
        store.path = path.to_path_buf();
        if !store.groups.iter().any(|g| g.id == DEFAULT_GROUP) {
            store.groups.insert(
                0,
                Group {
                    id: DEFAULT_GROUP,
                    name: "Default".into(),
                    subscription: None,
                },
            );
        }
        Ok(store)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        write_atomic(&self.path, &serde_json::to_vec_pretty(self)?)
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    // ---------------------------------------------------------------- groups

    pub fn group(&self, id: GroupId) -> Option<&Group> {
        self.groups.iter().find(|g| g.id == id)
    }

    pub fn group_mut(&mut self, id: GroupId) -> Option<&mut Group> {
        self.groups.iter_mut().find(|g| g.id == id)
    }

    pub fn add_group(
        &mut self,
        name: impl Into<String>,
        subscription: Option<Subscription>,
    ) -> GroupId {
        let id = self.alloc_id();
        self.groups.push(Group {
            id,
            name: name.into(),
            subscription,
        });
        id
    }

    /// Removes a group with all its profiles. The default group cannot be removed.
    pub fn remove_group(&mut self, id: GroupId) -> bool {
        if id == DEFAULT_GROUP {
            return false;
        }
        self.profiles.retain(|p| p.group != id);
        let before = self.groups.len();
        self.groups.retain(|g| g.id != id);
        before != self.groups.len()
    }

    // ---------------------------------------------------------------- profiles

    pub fn profile(&self, id: ProfileId) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    pub fn profile_mut(&mut self, id: ProfileId) -> Option<&mut Profile> {
        self.profiles.iter_mut().find(|p| p.id == id)
    }

    pub fn profiles_in(&self, group: GroupId) -> impl Iterator<Item = &Profile> {
        self.profiles.iter().filter(move |p| p.group == group)
    }

    /// Adds profiles to `group`, skipping exact duplicates. Returns the new ids.
    pub fn add_profiles(&mut self, group: GroupId, profiles: Vec<Profile>) -> Vec<ProfileId> {
        let mut ids = Vec::new();
        for mut p in profiles {
            let key = p.dedup_key();
            if self.profiles_in(group).any(|e| e.dedup_key() == key) {
                continue;
            }
            p.id = self.alloc_id();
            p.group = group;
            ids.push(p.id);
            self.profiles.push(p);
        }
        ids
    }

    /// Replaces the contents of a subscription group. Profiles that are unchanged keep
    /// their ids (so the selection and latency survive an update).
    pub fn replace_group_profiles(&mut self, group: GroupId, profiles: Vec<Profile>) -> usize {
        let old: Vec<Profile> = self
            .profiles
            .iter()
            .filter(|p| p.group == group)
            .cloned()
            .collect();
        self.profiles.retain(|p| p.group != group);
        let count = profiles.len();
        for mut p in profiles {
            let key = p.dedup_key();
            match old
                .iter()
                .find(|o| o.dedup_key() == key && o.name == p.name)
            {
                Some(o) => {
                    p.id = o.id;
                    p.latency = o.latency.clone();
                }
                None => p.id = self.alloc_id(),
            }
            p.group = group;
            self.profiles.push(p);
        }
        count
    }

    pub fn remove_profiles(&mut self, ids: &[ProfileId]) {
        self.profiles.retain(|p| !ids.contains(&p.id));
    }

    /// Adds a copy of an existing profile. Returns the new id.
    pub fn duplicate_profile(&mut self, id: ProfileId) -> Option<ProfileId> {
        let mut copy = self.profile(id)?.clone();
        copy.id = self.alloc_id();
        copy.name = format!("{} (copy)", copy.display_name());
        let new_id = copy.id;
        self.profiles.push(copy);
        Some(new_id)
    }

    pub fn set_latency(&mut self, id: ProfileId, latency: Latency) {
        if let Some(p) = self.profile_mut(id) {
            p.latency = Some(latency);
        }
    }

    /// Reorders a group to match `order` (ids not listed keep their relative order at the end).
    pub fn reorder(&mut self, group: GroupId, order: &[ProfileId]) {
        let mut members: Vec<Profile> = self
            .profiles
            .iter()
            .filter(|p| p.group == group)
            .cloned()
            .collect();
        members.sort_by_key(|p| {
            order
                .iter()
                .position(|id| *id == p.id)
                .unwrap_or(usize::MAX)
        });
        let mut members = members.into_iter();
        for slot in self.profiles.iter_mut().filter(|p| p.group == group) {
            *slot = members.next().expect("same length");
        }
    }

    /// Sorts a group by latency (fastest first, failures last).
    pub fn sort_by_latency(&mut self, group: GroupId) {
        let rank = |p: &Profile| match &p.latency {
            Some(Latency::Ms(ms)) => *ms as u64,
            Some(Latency::Error(_)) => u64::MAX - 1,
            None => u64::MAX,
        };
        let mut members: Vec<Profile> = self
            .profiles
            .iter()
            .filter(|p| p.group == group)
            .cloned()
            .collect();
        members.sort_by_key(rank);
        let mut members = members.into_iter();
        for slot in self.profiles.iter_mut().filter(|p| p.group == group) {
            *slot = members.next().expect("same length");
        }
    }
}

/// Writes a file atomically (temp file + rename), creating parent directories.
pub fn write_atomic(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data).with_context(|| format!("failed to write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("failed to replace {}", path.display()))?;
    Ok(())
}
