//! The registry's memory: the services, and the namespaces.
//!
//! Generic over what a handle is, so that the host tests can use
//! numbers where the program uses kernel handles. The table never
//! closes or copies a handle: whatever it stops holding it gives back,
//! and the program does what must be done with it.

use alloc::string::String;
use alloc::vec::Vec;

use crate::protocol::valid_name;

/// Most services registered at once.
pub const SERVICES_MAX: usize = 32;
/// Most namespaces. The program waits on every client channel and the
/// admin channel at once, and the kernel waits on 32 things at most.
pub const NAMESPACES_MAX: usize = 30;
/// Most names in one namespace.
pub const NAMES_MAX: usize = 16;

struct Service<H> {
    name: String,
    handle: H,
    /// The rights a client's copy is to have.
    rights: u32,
}

struct Namespace<H> {
    label: String,
    /// The names that exist here, each with the service it means - the
    /// name it was registered under. In a namespace init made the two
    /// are the same; in a narrowed one they need not be.
    names: Vec<(String, String)>,
    /// The receiving end of the client's channel.
    channel: H,
    /// Whether a client asked for it (NARROW), and not init. Such a
    /// one is never replaced by label: two programs may be called `ls`.
    narrowed: bool,
}

/// Why a lookup found nothing. The client is told only "not found"
/// either way; the difference is for the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Miss {
    /// The name is not in the client's namespace.
    NotInNamespace,
    /// It is, but nothing is registered under it - the service has not
    /// started, or the machine has no such device.
    NotRegistered,
}

pub struct Table<H> {
    services: Vec<Service<H>>,
    namespaces: Vec<Namespace<H>>,
}

// Written out, not derived: `#[derive(Default)]` would demand that H
// have a default too, and an empty table holds no H at all.
impl<H> Default for Table<H> {
    fn default() -> Self {
        Table { services: Vec::new(), namespaces: Vec::new() }
    }
}

impl<H> Table<H> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes `handle` what `name` means. `Ok(Some(old))` if the name
    /// meant something before - the old handle, no longer held.
    /// `Err(handle)` if the name is not valid or the table is full:
    /// the handle was not taken.
    pub fn register(&mut self, name: &str, handle: H, rights: u32) -> Result<Option<H>, H> {
        if !valid_name(name) {
            return Err(handle);
        }
        if let Some(service) = self.services.iter_mut().find(|service| service.name == name) {
            service.rights = rights;
            // `replace` puts the new handle in and returns the old.
            return Ok(Some(core::mem::replace(&mut service.handle, handle)));
        }
        if self.services.len() == SERVICES_MAX {
            return Err(handle);
        }
        self.services.push(Service { name: String::from(name), handle, rights });
        Ok(None)
    }

    /// Adds a namespace: a client's channel, and the names that exist
    /// on it. `Ok(Some(old))` if there was a namespace with this label:
    /// it is replaced, and its channel is no longer held.
    /// `Err(channel)` if a name is not valid, there are too many, or
    /// the table is full.
    pub fn add_namespace(&mut self, label: &str, names: &[&str], channel: H) -> Result<Option<H>, H> {
        if !valid_name(label) || names.len() > NAMES_MAX || !names.iter().all(|name| valid_name(name)) {
            return Err(channel);
        }
        let names = names.iter().map(|&name| (String::from(name), String::from(name))).collect();
        if let Some(namespace) = self.namespaces.iter_mut().find(|namespace| !namespace.narrowed && namespace.label == label) {
            namespace.names = names;
            return Ok(Some(core::mem::replace(&mut namespace.channel, channel)));
        }
        if self.namespaces.len() == NAMESPACES_MAX {
            return Err(channel);
        }
        self.namespaces.push(Namespace { label: String::from(label), names, channel, narrowed: false });
        Ok(None)
    }

    /// Adds a namespace made from the one at `index`: each of `names`
    /// is a name it is to have, and the name in the namespace at
    /// `index` that it is to mean. `Err(channel)` if one of those is
    /// not there - a client cannot give away what it cannot find - or
    /// a name is not valid, there are too many, or the table is full.
    ///
    /// What is recorded is the *service* each name means, looked up
    /// now: the new namespace does not depend on the old one staying.
    pub fn narrow(&mut self, index: usize, label: &str, names: &[(&str, &str)], channel: H) -> Result<(), H> {
        let Some(from) = self.namespaces.get(index) else { return Err(channel) };
        if !valid_name(label) || names.len() > NAMES_MAX || self.namespaces.len() == NAMESPACES_MAX {
            return Err(channel);
        }
        let mut meant = Vec::new();
        for &(new, old) in names {
            let found = from.names.iter().find(|(name, _)| name == old);
            match found {
                Some((_, service)) if valid_name(new) => meant.push((String::from(new), service.clone())),
                _ => return Err(channel),
            }
        }
        self.namespaces.push(Namespace { label: String::from(label), names: meant, channel, narrowed: true });
        Ok(())
    }

    /// Forgets the namespace at `index` - its client has gone - and
    /// returns its channel. The namespaces after it move down one.
    pub fn remove_namespace(&mut self, index: usize) -> Option<H> {
        (index < self.namespaces.len()).then(|| self.namespaces.remove(index).channel)
    }

    /// The service `name`, as seen from the namespace at `index`: its
    /// handle, and the rights a copy is to carry.
    pub fn lookup(&self, index: usize, name: &str) -> Result<(&H, u32), Miss> {
        let meant = self.namespaces.get(index).and_then(|namespace| namespace.names.iter().find(|(allowed, _)| allowed == name));
        let Some((_, meant)) = meant else { return Err(Miss::NotInNamespace) };
        self.services
            .iter()
            .find(|service| service.name == *meant)
            .map(|service| (&service.handle, service.rights))
            .ok_or(Miss::NotRegistered)
    }

    /// The names a lookup from the namespace at `index` would find, in
    /// the namespace's order.
    pub fn list(&self, index: usize) -> Vec<&str> {
        let names = self.namespaces.get(index).map_or(&[][..], |namespace| &namespace.names[..]);
        names
            .iter()
            .filter(|(_, meant)| self.services.iter().any(|service| service.name == *meant))
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// Whose namespace the one at `index` is.
    pub fn label(&self, index: usize) -> &str {
        self.namespaces.get(index).map_or("?", |namespace| namespace.label.as_str())
    }

    /// The client channels, in the order of their indexes.
    pub fn channels(&self) -> impl Iterator<Item = &H> {
        self.namespaces.iter().map(|namespace| &namespace.channel)
    }

    pub fn service_count(&self) -> usize {
        self.services.len()
    }

    pub fn namespace_count(&self) -> usize {
        self.namespaces.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A table as init would fill it: handles are numbers here.
    fn filled() -> Table<u32> {
        let mut table = Table::new();
        assert_eq!(table.register("files", 10, 1), Ok(None));
        assert_eq!(table.register("files.look", 11, 1), Ok(None));
        assert_eq!(table.register("disk", 12, 3), Ok(None));
        assert_eq!(table.add_namespace("shell", &["files", "files.look", "display"], 100), Ok(None));
        assert_eq!(table.add_namespace("fs", &["disk"], 101), Ok(None));
        table
    }

    #[test]
    fn a_client_finds_what_is_in_its_namespace() {
        let table = filled();
        assert_eq!(table.lookup(0, "files"), Ok((&10, 1)));
        assert_eq!(table.lookup(0, "files.look"), Ok((&11, 1)));
        assert_eq!(table.lookup(1, "disk"), Ok((&12, 3)));
    }

    #[test]
    fn a_name_outside_the_namespace_is_not_there() {
        let table = filled();
        // The disk is registered - but not for the shell.
        assert_eq!(table.lookup(0, "disk"), Err(Miss::NotInNamespace));
        assert_eq!(table.lookup(1, "files"), Err(Miss::NotInNamespace));
        assert_eq!(table.lookup(0, "nosuch"), Err(Miss::NotInNamespace));
        assert_eq!(table.lookup(0, ""), Err(Miss::NotInNamespace));
        // And a namespace that does not exist has nothing in it.
        assert_eq!(table.lookup(7, "files"), Err(Miss::NotInNamespace));
    }

    #[test]
    fn a_name_allowed_but_not_registered_is_not_there_either() {
        let mut table = filled();
        assert_eq!(table.lookup(0, "display"), Err(Miss::NotRegistered));
        // Until it is: services may register after the namespace is made.
        assert_eq!(table.register("display", 13, 1), Ok(None));
        assert_eq!(table.lookup(0, "display"), Ok((&13, 1)));
    }

    #[test]
    fn the_list_is_what_a_lookup_would_find() {
        let mut table = filled();
        assert_eq!(table.list(0), ["files", "files.look"]);
        assert_eq!(table.list(1), ["disk"]);
        assert!(table.list(7).is_empty());
        table.register("display", 13, 1).unwrap();
        assert_eq!(table.list(0), ["files", "files.look", "display"]);
    }

    #[test]
    fn registering_again_replaces_and_returns_the_old_handle() {
        let mut table = filled();
        assert_eq!(table.register("files", 20, 7), Ok(Some(10)));
        assert_eq!(table.lookup(0, "files"), Ok((&20, 7)));
        assert_eq!(table.service_count(), 3);
    }

    #[test]
    fn a_namespace_with_a_label_in_use_replaces_it() {
        let mut table = filled();
        assert_eq!(table.add_namespace("shell", &["disk"], 200), Ok(Some(100)));
        assert_eq!(table.namespace_count(), 2);
        // Its place - its index - is kept; its names are the new ones.
        assert_eq!(table.label(0), "shell");
        assert_eq!(table.lookup(0, "disk"), Ok((&12, 3)));
        assert_eq!(table.lookup(0, "files"), Err(Miss::NotInNamespace));
        assert_eq!(table.channels().copied().collect::<Vec<_>>(), [200, 101]);
    }

    #[test]
    fn a_removed_namespace_gives_its_channel_back() {
        let mut table = filled();
        assert_eq!(table.remove_namespace(0), Some(100));
        assert_eq!(table.remove_namespace(5), None);
        // The one after it moved down.
        assert_eq!(table.label(0), "fs");
        assert_eq!(table.lookup(0, "disk"), Ok((&12, 3)));
        assert_eq!(table.namespace_count(), 1);
    }

    #[test]
    fn a_narrowed_namespace_has_fewer_names_and_may_rename_them() {
        let mut table = filled();
        // The shell (0) makes one for `ls`: "files", meaning files.look.
        assert_eq!(table.narrow(0, "ls", &[("files", "files.look")], 300), Ok(()));
        assert_eq!(table.label(2), "ls");
        assert_eq!(table.lookup(2, "files"), Ok((&11, 1))); // the read-only channel
        assert_eq!(table.lookup(2, "files.look"), Err(Miss::NotInNamespace));
        assert_eq!(table.lookup(2, "disk"), Err(Miss::NotInNamespace));
        assert_eq!(table.list(2), ["files"]);
        // And the shell's own is as it was.
        assert_eq!(table.lookup(0, "files"), Ok((&10, 1)));
    }

    #[test]
    fn nobody_can_give_what_they_cannot_find() {
        let mut table = filled();
        // The disk is registered - but not in the shell's namespace.
        assert_eq!(table.narrow(0, "sneak", &[("disk", "disk")], 300), Err(300));
        assert_eq!(table.narrow(0, "sneak", &[("files", "files"), ("x", "nosuch")], 301), Err(301));
        assert_eq!(table.narrow(9, "sneak", &[], 302), Err(302)); // no such namespace
        assert_eq!(table.namespace_count(), 2);
        // Through a narrowed one, only what it was given.
        table.narrow(0, "ls", &[("files", "files.look")], 303).unwrap();
        assert_eq!(table.narrow(2, "child", &[("files", "files")], 304), Ok(()));
        assert_eq!(table.lookup(3, "files"), Ok((&11, 1))); // still the read-only one
        assert_eq!(table.narrow(2, "child", &[("more", "files.look")], 305), Err(305));
    }

    #[test]
    fn a_narrowed_name_may_be_allowed_before_it_is_registered() {
        let mut table = filled();
        assert_eq!(table.narrow(0, "early", &[("screen", "display")], 300), Ok(()));
        assert_eq!(table.lookup(2, "screen"), Err(Miss::NotRegistered));
        assert!(table.list(2).is_empty());
        table.register("display", 13, 1).unwrap();
        assert_eq!(table.lookup(2, "screen"), Ok((&13, 1)));
        assert_eq!(table.list(2), ["screen"]);
    }

    #[test]
    fn narrowed_namespaces_are_never_replaced_by_label() {
        let mut table = filled();
        table.narrow(0, "ls", &[("files", "files.look")], 300).unwrap();
        table.narrow(0, "ls", &[("files", "files")], 301).unwrap();
        assert_eq!(table.namespace_count(), 4);
        assert_eq!(table.lookup(2, "files"), Ok((&11, 1)));
        assert_eq!(table.lookup(3, "files"), Ok((&10, 1)));
        // A program that calls itself "shell" does not become init's
        // shell: init's is replaced, the program's is left alone.
        table.narrow(0, "shell", &[], 302).unwrap();
        assert_eq!(table.add_namespace("shell", &["disk"], 400), Ok(Some(100)));
        assert_eq!(table.namespace_count(), 5);
        assert_eq!(table.lookup(0, "disk"), Ok((&12, 3)));
        assert!(table.list(4).is_empty());
        // It still means what it meant: the service, not the shell's name for it.
        assert_eq!(table.lookup(2, "files"), Ok((&11, 1)));
    }

    #[test]
    fn a_bad_narrowing_hands_the_channel_back() {
        let mut table = filled();
        assert_eq!(table.narrow(0, "BAD", &[], 1), Err(1));
        assert_eq!(table.narrow(0, "ok", &[("BAD", "files")], 2), Err(2));
        let many: Vec<(&str, &str)> = core::iter::repeat_n(("files", "files"), NAMES_MAX + 1).collect();
        assert_eq!(table.narrow(0, "many", &many, 3), Err(3));
        for n in 2..NAMESPACES_MAX {
            assert_eq!(table.narrow(0, "p", &[], n as u32), Ok(()));
        }
        assert_eq!(table.narrow(0, "p", &[], 999), Err(999));
    }

    #[test]
    fn what_is_not_taken_is_handed_back() {
        let mut table = filled();
        assert_eq!(table.register("Not A Name", 1, 0), Err(1));
        assert_eq!(table.add_namespace("ok", &["BAD"], 2), Err(2));
        assert_eq!(table.add_namespace("BAD", &[], 3), Err(3));
        let many: Vec<&str> = core::iter::repeat_n("x", NAMES_MAX + 1).collect();
        assert_eq!(table.add_namespace("many", &many, 4), Err(4));
        // A namespace with no names is fine: it can ask for nothing.
        assert_eq!(table.add_namespace("nobody", &[], 5), Ok(None));
        assert!(table.list(2).is_empty());
    }

    #[test]
    fn a_full_table_takes_no_more_but_still_replaces() {
        let mut table: Table<u32> = Table::new();
        for n in 0..SERVICES_MAX {
            assert_eq!(table.register(&format!("s{n}"), n as u32, 0), Ok(None));
        }
        assert_eq!(table.register("one-more", 999, 0), Err(999));
        assert_eq!(table.register("s0", 500, 0), Ok(Some(0)));
        for n in 0..NAMESPACES_MAX {
            assert_eq!(table.add_namespace(&format!("n{n}"), &[], n as u32), Ok(None));
        }
        assert_eq!(table.add_namespace("one-more", &[], 999), Err(999));
        assert_eq!(table.add_namespace("n0", &["s1"], 500), Ok(Some(0)));
    }
}
