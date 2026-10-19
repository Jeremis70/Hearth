//! Blocking D-Bus client for the InputPlumber daemon (verified against 0.82.0, see
//! `docs/spikes/inputplumber.md`).
//!
//! Every write is a polkit `auth_admin` action in InputPlumber; members of `wheel` or
//! `inputplumber` are allowed without a prompt by its shipped rules. Reads are open to anyone.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use zbus::fdo::ObjectManagerProxy;
use zbus::{Connection, OwnedObjectPath, OwnedVariant};

use crate::input::{Connection as PadConnection, PadInfo, Power};

/// Well-known bus name of the daemon.
pub const SERVICE: &str = "org.shadowblip.InputPlumber";
/// Root of the daemon's object tree (it serves `org.freedesktop.DBus.ObjectManager` there).
const ROOT: &str = "/org/shadowblip/InputPlumber";
/// Interface of a composite device (one per physical controller).
pub const COMPOSITE_INTERFACE: &str = "org.shadowblip.Input.CompositeDevice";
/// The profile every composite device starts with; loading it is InputPlumber's only "unload".
pub const DEFAULT_PROFILE: &str = "/usr/share/inputplumber/profiles/default.yaml";
/// Oldest version with polkit checks and the fixes for CVE-2025-66005 / CVE-2025-14338.
pub const MIN_VERSION: (u32, u32) = (0, 69);

#[zbus::proxy(
    interface = "org.shadowblip.InputManager",
    default_service = "org.shadowblip.InputPlumber",
    default_path = "/org/shadowblip/InputPlumber/Manager"
)]
trait Manager {
    #[zbus(property)]
    fn version(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn manage_all_devices(&self) -> zbus::Result<bool>;

    #[zbus(property)]
    fn set_manage_all_devices(&self, value: bool) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.shadowblip.Input.CompositeDevice",
    default_service = "org.shadowblip.InputPlumber"
)]
trait CompositeDevice {
    fn load_profile_path(&self, path: &str) -> zbus::Result<()>;

    fn set_target_devices(&self, target_device_types: &[&str]) -> zbus::Result<()>;

    #[zbus(property)]
    fn name(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn persistent_id(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn intercept_mode(&self) -> zbus::Result<u32>;

    #[zbus(property)]
    fn set_intercept_mode(&self, value: u32) -> zbus::Result<()>;

    #[zbus(property)]
    fn profile_path(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn source_device_paths(&self) -> zbus::Result<Vec<String>>;

    #[zbus(property)]
    fn dbus_devices(&self) -> zbus::Result<Vec<String>>;

    #[zbus(property)]
    fn capabilities(&self) -> zbus::Result<Vec<String>>;

    #[zbus(property)]
    fn target_devices(&self) -> zbus::Result<Vec<String>>;
}

#[zbus::proxy(
    interface = "org.shadowblip.Input.Source.UdevDevice",
    default_service = "org.shadowblip.InputPlumber"
)]
trait UdevDevice {
    #[zbus(property)]
    fn name(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn id_vendor(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn id_product(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn id_bustype(&self) -> zbus::Result<String>;
}

/// Who receives a composite device's input (`InterceptMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterceptMode {
    /// Everything goes to the virtual controller.
    None = 0,
    /// Everything goes to the virtual controller except Guide, which switches the device to
    /// `Always` by itself and is sent over D-Bus.
    Pass = 1,
    /// Everything goes over D-Bus; the virtual controller sees nothing.
    Always = 2,
}

impl InterceptMode {
    fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            0 => Self::None,
            1 => Self::Pass,
            2 => Self::Always,
            _ => return None,
        })
    }
}

/// Errors from the InputPlumber client.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// D-Bus failed (no system bus, daemon gone, polkit refused, …).
    #[error("D-Bus: {0}")]
    DBus(String),
    /// The daemon isn't running.
    #[error("InputPlumber is not running")]
    NotRunning,
    /// The daemon is older than [`MIN_VERSION`].
    #[error("InputPlumber {0} is too old (need 0.69 or newer)")]
    TooOld(String),
}

impl From<zbus::Error> for ClientError {
    fn from(err: zbus::Error) -> Self {
        Self::DBus(err.to_string())
    }
}

impl From<zbus::fdo::Error> for ClientError {
    fn from(err: zbus::fdo::Error) -> Self {
        Self::DBus(err.to_string())
    }
}

/// A connection to the InputPlumber daemon on the system bus.
#[derive(Debug, Clone)]
pub struct Client {
    conn: Connection,
}

impl Client {
    /// Connects to the system bus and checks that the daemon runs and is recent enough.
    pub fn connect() -> Result<Self, ClientError> {
        let conn = zbus::block_on(Connection::system())?;
        let dbus = zbus::block_on(zbus::fdo::DBusProxy::new(&conn))?;
        let name = zbus::names::BusName::try_from(SERVICE)?;
        if !zbus::block_on(dbus.name_has_owner(name))? {
            return Err(ClientError::NotRunning);
        }
        let client = Self { conn };
        let version = client.version()?;
        if !version_at_least(&version, MIN_VERSION) {
            return Err(ClientError::TooOld(version));
        }
        Ok(client)
    }

    /// The underlying connection (for signal subscriptions).
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    fn manager(&self) -> Result<ManagerProxy<'_>, ClientError> {
        Ok(zbus::block_on(ManagerProxy::new(&self.conn))?)
    }

    fn device(&self, path: &str) -> Result<CompositeDeviceProxy<'_>, ClientError> {
        Ok(zbus::block_on(
            CompositeDeviceProxy::builder(&self.conn)
                .path(path.to_owned())
                // Properties change behind our back (Guide switches Pass to Always without a
                // PropertiesChanged signal), so always read them fresh.
                .cache_properties(zbus::proxy::CacheProperties::No)
                .build(),
        )?)
    }

    /// The daemon's version ("0.82.0").
    pub fn version(&self) -> Result<String, ClientError> {
        Ok(zbus::block_on(self.manager()?.version())?)
    }

    /// Whether InputPlumber manages every supported controller, not only built-in ones.
    pub fn manage_all(&self) -> Result<bool, ClientError> {
        Ok(zbus::block_on(self.manager()?.manage_all_devices())?)
    }

    /// See [`Client::manage_all`]. Turning it off releases external controllers (their raw
    /// devices come back). Held in the daemon's memory only.
    pub fn set_manage_all(&self, on: bool) -> Result<(), ClientError> {
        Ok(zbus::block_on(self.manager()?.set_manage_all_devices(on))?)
    }

    /// Object paths of every composite device.
    pub fn composite_devices(&self) -> Result<Vec<String>, ClientError> {
        let objects = ObjectManagerProxy::builder(&self.conn)
            .destination(SERVICE)
            .path(ROOT)
            .build();
        let objects = zbus::block_on(async { objects.await?.get_managed_objects().await })?;
        Ok(composite_paths(&objects))
    }

    /// A composite device's intercept mode (`None` for values Hearth doesn't use).
    pub fn intercept_mode(&self, device: &str) -> Result<Option<InterceptMode>, ClientError> {
        Ok(InterceptMode::from_raw(zbus::block_on(
            self.device(device)?.intercept_mode(),
        )?))
    }

    /// Sets who receives a composite device's input.
    pub fn set_intercept_mode(&self, device: &str, mode: InterceptMode) -> Result<(), ClientError> {
        Ok(zbus::block_on(
            self.device(device)?.set_intercept_mode(mode as u32),
        )?)
    }

    /// Loads a profile file. The daemon (root, its own working directory) opens it, so the path
    /// is made absolute first.
    pub fn load_profile(&self, device: &str, profile: &Path) -> Result<(), ClientError> {
        let profile = std::path::absolute(profile)
            .map_err(|err| ClientError::DBus(format!("{}: {err}", profile.display())))?;
        Ok(zbus::block_on(
            self.device(device)?
                .load_profile_path(&profile.to_string_lossy()),
        )?)
    }

    /// The path of the profile a composite device has loaded.
    pub fn profile_path(&self, device: &str) -> Result<String, ClientError> {
        Ok(zbus::block_on(self.device(device)?.profile_path())?)
    }

    /// Replaces a composite device's virtual devices (`"xbox-elite"`, `"keyboard"`, …).
    pub fn set_targets(&self, device: &str, targets: &[&str]) -> Result<(), ClientError> {
        Ok(zbus::block_on(
            self.device(device)?.set_target_devices(targets),
        )?)
    }

    /// The inputs a composite device produces ("Gamepad:Button:South", …). Empty for a device
    /// that only holds blocked sources: InputPlumber 0.82 sometimes puts a DualSense's evdev
    /// nodes in a composite device of their own next to the real one (a race when it takes
    /// the controller).
    pub fn capabilities(&self, device: &str) -> Result<Vec<String>, ClientError> {
        Ok(zbus::block_on(self.device(device)?.capabilities())?)
    }

    /// The virtual devices a composite device feeds (object paths).
    pub fn targets(&self, device: &str) -> Result<Vec<String>, ClientError> {
        Ok(zbus::block_on(self.device(device)?.target_devices())?)
    }

    /// The D-Bus target objects that emit a composite device's intercepted input.
    pub fn dbus_targets(&self, device: &str) -> Result<Vec<String>, ClientError> {
        Ok(zbus::block_on(self.device(device)?.dbus_devices())?)
    }

    /// Hands a composite device back: input to its virtual devices, default profile.
    pub fn release(&self, device: &str) -> Result<(), ClientError> {
        self.set_intercept_mode(device, InterceptMode::None)?;
        self.load_profile(device, Path::new(DEFAULT_PROFILE))
    }

    /// The physical controller behind a composite device, from its source devices (the virtual
    /// pad Hearth asks for is an Xbox pad; glyphs must follow the real one).
    pub fn source_pad(&self, device: &str) -> Result<PadInfo, ClientError> {
        let composite = self.device(device)?;
        let mut sources = Vec::new();
        for dev_path in zbus::block_on(composite.source_device_paths())? {
            let Some(name) = source_object(&dev_path) else {
                continue;
            };
            let udev = zbus::block_on(
                UdevDeviceProxy::builder(&self.conn)
                    .path(name)
                    .cache_properties(zbus::proxy::CacheProperties::No)
                    .build(),
            )?;
            sources.push(SourceInfo {
                dev_path,
                name: zbus::block_on(udev.name()).unwrap_or_default(),
                vendor: zbus::block_on(udev.id_vendor()).unwrap_or_default(),
                product: zbus::block_on(udev.id_product()).unwrap_or_default(),
                bus: zbus::block_on(udev.id_bustype()).unwrap_or_default(),
            });
        }
        let fallback_name = zbus::block_on(composite.name()).unwrap_or_default();
        let mut pad = pad_info(&sources, &fallback_name);
        pad.id = match zbus::block_on(composite.persistent_id()) {
            Ok(id) if !id.is_empty() => format!("inputplumber:{id}"),
            _ => format!("inputplumber:{device}"),
        };
        Ok(pad)
    }
}

/// What a source device object says about itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct SourceInfo {
    /// Device node ("/dev/hidraw6", "/dev/input/event22").
    dev_path: String,
    name: String,
    /// Lowercase hex ("054c").
    vendor: String,
    product: String,
    /// Decimal Linux bus type ("3" USB, "5" Bluetooth).
    bus: String,
}

/// Object path of the source device behind a device node.
fn source_object(dev_path: &str) -> Option<String> {
    let base = Path::new(dev_path).file_name()?.to_str()?;
    let valid = !base.is_empty() && base.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    valid.then(|| format!("{ROOT}/devices/source/{base}"))
}

/// Builds a [`PadInfo`] from a controller's source devices. The name is the shortest evdev
/// name, the base device ("DualSense Wireless Controller", next to "… Touchpad" and "… Motion
/// Sensors"); hidraw names can be any of those, so without evdev sources the composite
/// device's name is used.
fn pad_info(sources: &[SourceInfo], fallback_name: &str) -> PadInfo {
    let hex = |s: &str| u16::from_str_radix(s, 16).ok().filter(|v| *v != 0);
    let name = sources
        .iter()
        .filter(|s| s.dev_path.contains("/input/event"))
        .map(|s| s.name.as_str())
        .filter(|n| !n.is_empty())
        .min_by_key(|n| n.len())
        .unwrap_or(fallback_name)
        .to_owned();
    let with_ids = sources.iter().find(|s| hex(&s.vendor).is_some());
    let bus = sources.iter().find_map(|s| s.bus.parse::<u16>().ok());
    let hidraw = sources.iter().find(|s| s.dev_path.contains("hidraw"));
    PadInfo {
        id: String::new(),
        name: name.clone(),
        os_name: name,
        vendor_id: with_ids.and_then(|s| hex(&s.vendor)),
        product_id: with_ids.and_then(|s| hex(&s.product)),
        connection: bus.map_or(PadConnection::Other, PadConnection::from_bus),
        power: hidraw.map_or(Power::Unknown, |s| sysfs_power(&s.dev_path)),
    }
}

/// Battery of a hidraw device from sysfs (`hid-playstation` and friends register a
/// power_supply under the HID device).
fn sysfs_power(dev_path: &str) -> Power {
    let Some(base) = Path::new(dev_path).file_name() else {
        return Power::Unknown;
    };
    let dir = Path::new("/sys/class/hidraw")
        .join(base)
        .join("device/power_supply");
    let Some(supply) = std::fs::read_dir(dir)
        .ok()
        .and_then(|mut entries| entries.next())
        .and_then(Result::ok)
    else {
        return Power::Unknown;
    };
    let read = |file: &str| std::fs::read_to_string(supply.path().join(file)).unwrap_or_default();
    power_from(read("status").trim(), read("capacity").trim().parse().ok())
}

/// A power_supply `status` and `capacity` as a [`Power`].
fn power_from(status: &str, capacity: Option<u8>) -> Power {
    match (status, capacity) {
        ("Full", _) => Power::Charged,
        ("Charging", Some(p)) => Power::Charging(p),
        ("Discharging" | "Not charging", Some(p)) => Power::Discharging(p),
        _ => Power::Unknown,
    }
}

/// Composite device paths in an ObjectManager listing, in name order (CompositeDevice0 first).
fn composite_paths(
    objects: &HashMap<
        OwnedObjectPath,
        BTreeMap<zbus::names::OwnedInterfaceName, HashMap<String, OwnedVariant>>,
    >,
) -> Vec<String> {
    let mut paths: Vec<String> = objects
        .iter()
        .filter(|(_, ifaces)| ifaces.keys().any(|i| i.as_str() == COMPOSITE_INTERFACE))
        .map(|(path, _)| path.to_string())
        .collect();
    paths.sort();
    paths
}

/// Whether a "major.minor.patch" version is at least `min`.
fn version_at_least(version: &str, min: (u32, u32)) -> bool {
    let mut parts = version.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let major = parts.next().unwrap_or(0);
    let minor = parts.next().unwrap_or(0);
    (major, minor) >= min
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(dev: &str, name: &str, bus: &str) -> SourceInfo {
        SourceInfo {
            dev_path: dev.into(),
            name: name.into(),
            vendor: "054c".into(),
            product: "0ce6".into(),
            bus: bus.into(),
        }
    }

    #[test]
    fn versions() {
        assert!(version_at_least("0.82.0", MIN_VERSION));
        assert!(version_at_least("0.69.0", MIN_VERSION));
        assert!(version_at_least("1.0.0", MIN_VERSION));
        assert!(!version_at_least("0.68.3", MIN_VERSION));
        assert!(!version_at_least("garbage", MIN_VERSION));
    }

    #[test]
    fn source_objects_from_device_nodes() {
        assert_eq!(
            source_object("/dev/hidraw6").as_deref(),
            Some("/org/shadowblip/InputPlumber/devices/source/hidraw6")
        );
        assert_eq!(
            source_object("/dev/input/event22").as_deref(),
            Some("/org/shadowblip/InputPlumber/devices/source/event22")
        );
        assert_eq!(source_object(""), None);
        assert_eq!(source_object("/dev/bad-name"), None);
    }

    #[test]
    fn dualsense_identity_from_sources() {
        // As InputPlumber 0.82 reports a DualSense over Bluetooth.
        let sources = [
            source(
                "/dev/hidraw6",
                "DualSense Wireless Controller Touchpad",
                "5",
            ),
            source("/dev/input/event22", "DualSense Wireless Controller", "5"),
            source(
                "/dev/input/event23",
                "DualSense Wireless Controller Motion Sensors",
                "5",
            ),
        ];
        let pad = pad_info(
            &sources,
            "Sony Interactive Entertainment DualSense Wireless Controller",
        );
        assert_eq!(pad.os_name, "DualSense Wireless Controller");
        assert_eq!(pad.vendor_id, Some(0x054c));
        assert_eq!(pad.product_id, Some(0x0ce6));
        assert_eq!(pad.connection, PadConnection::Bluetooth);
    }

    #[test]
    fn hidraw_only_controller_uses_the_composite_name() {
        let sources = [source(
            "/dev/hidraw6",
            "DualSense Wireless Controller Touchpad",
            "5",
        )];
        let pad = pad_info(
            &sources,
            "Sony Interactive Entertainment DualSense Wireless Controller",
        );
        assert_eq!(
            pad.os_name,
            "Sony Interactive Entertainment DualSense Wireless Controller"
        );
        assert_eq!(pad.vendor_id, Some(0x054c));
    }

    #[test]
    fn identity_falls_back_to_composite_name() {
        let pad = pad_info(&[], "Some Pad");
        assert_eq!(pad.os_name, "Some Pad");
        assert_eq!(pad.vendor_id, None);
        assert_eq!(pad.connection, PadConnection::Other);
    }

    #[test]
    fn battery_states() {
        assert_eq!(power_from("Discharging", Some(15)), Power::Discharging(15));
        assert_eq!(power_from("Charging", Some(80)), Power::Charging(80));
        assert_eq!(power_from("Full", Some(100)), Power::Charged);
        assert_eq!(power_from("", None), Power::Unknown);
    }
}
