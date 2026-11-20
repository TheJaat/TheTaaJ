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
    names: Vec<String>,
    /// The receiving end of the client's channel.
    channel: H,
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
        let names = names.iter().map(|&name| String::from(name)).collect();
        if let Some(namespace) = self.namespaces.iter_mut().find(|namespace| namespace.label == label) {
            namespace.names = names;
            return Ok(Some(core::mem::replace(&mut namespace.channel, channel)));
        }
        if self.namespaces.len() == NAMESPACES_MAX {
            return Err(channel);
        }
        self.namespaces.push(Namespace { label: String::from(label), names, channel });
        Ok(None)
    }

    /// Forgets the namespace at `index` - its client has gone - and
    /// returns its channel. The namespaces after it move down one.
    pub fn remove_namespace(&mut self, index: usize) -> Option<H> {
        (index < self.namespaces.len()).then(|| self.namespaces.remove(index).channel)
    }

    /// The service `name`, as seen from the namespace at `index`: its
    /// handle, and the rights a copy is to carry.
    pub fn lookup(&self, index: usize, name: &str) -> Result<(&H, u32), Miss> {
        let allowed = self.namespaces.get(index).is_some_and(|namespace| namespace.names.iter().any(|allowed| allowed == name));
        if !allowed {
            return Err(Miss::NotInNamespace);
        }
        self.services
            .iter()
            .find(|service| service.name == name)
            .map(|service| (&service.handle, service.rights))
            .ok_or(Miss::NotRegistered)
    }

    /// The names a lookup from the namespace at `index` would find, in
    /// the namespace's order.
    pub fn list(&self, index: usize) -> Vec<&str> {
        let names = self.namespaces.get(index).map_or(&[][..], |namespace| &namespace.names[..]);
        names
            .iter()
            .map(String::as_str)
            .filter(|name| self.services.iter().any(|service| service.name == *name))
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
