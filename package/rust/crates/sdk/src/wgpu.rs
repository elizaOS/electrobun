//! WebGPU (Dawn) interop.
//!
//! Ports the Zig `WgpuNative` / `WgpuContext`. [`WgpuNative`] opens a *second*
//! shared library (`libwebgpu_dawn`) and resolves the two wgpu entry points the
//! SDK needs. [`WgpuContext`] builds an instance/surface/adapter/device chain
//! for a native view, faithfully preserving the `[usize; 2]` adapter/device
//! out-array that core fills via `wgpuCreateAdapterDeviceMainThread`.

use std::os::raw::c_void;
use std::ptr;

use libloading::Library;

use crate::core::Core;
use crate::error::{ElectrobunError, Result};

/// `fn (descriptor) -> instance` — `wgpuCreateInstance`.
type CreateInstanceFn = extern "C" fn(*const c_void) -> *mut c_void;
/// `fn (device) -> queue` — `wgpuDeviceGetQueue`.
type DeviceGetQueueFn = extern "C" fn(*mut c_void) -> *mut c_void;

/// The platform-specific `libwebgpu_dawn` filename.
fn wgpu_lib_name() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "webgpu_dawn.dll"
    }
    #[cfg(target_os = "macos")]
    {
        "libwebgpu_dawn.dylib"
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        "libwebgpu_dawn.so"
    }
}

/// Handle to `libwebgpu_dawn` and its resolved wgpu symbols.
///
/// Like [`Core`], it owns its [`Library`]; the fn pointers stay valid only
/// while the library is loaded, so both drop together.
pub struct WgpuNative {
    create_instance: CreateInstanceFn,
    device_get_queue: DeviceGetQueueFn,
    #[allow(dead_code)]
    library: Library,
}

impl WgpuNative {
    /// Open `libwebgpu_dawn` from the bundle's executable directory and resolve
    /// the wgpu symbols the SDK uses.
    pub fn load() -> Result<WgpuNative> {
        let bundle_paths = crate::paths::resolve_bundle_paths()?;
        let lib_path = bundle_paths.exe_dir.join(wgpu_lib_name());
        // SAFETY: opening a shared library runs its initializers; the path is
        // the app's own bundled dawn build.
        let library =
            unsafe { Library::new(&lib_path) }.map_err(|source| ElectrobunError::LibraryOpen {
                path: lib_path.display().to_string(),
                source,
            })?;

        // SAFETY: signatures match dawn's C ABI.
        let create_instance = unsafe { library.get::<CreateInstanceFn>(b"wgpuCreateInstance\0") }
            .map_err(|_| ElectrobunError::MissingCoreSymbol {
            symbol: "wgpuCreateInstance",
        })?;
        let create_instance = *create_instance;

        // SAFETY: signatures match dawn's C ABI.
        let device_get_queue = unsafe { library.get::<DeviceGetQueueFn>(b"wgpuDeviceGetQueue\0") }
            .map_err(|_| ElectrobunError::MissingCoreSymbol {
                symbol: "wgpuDeviceGetQueue",
            })?;
        let device_get_queue = *device_get_queue;

        Ok(WgpuNative {
            create_instance,
            device_get_queue,
            library,
        })
    }

    /// Create a wgpu instance (with a null descriptor).
    pub fn create_instance(&self) -> *mut c_void {
        (self.create_instance)(ptr::null())
    }

    /// Get the default queue for a device.
    pub fn device_get_queue(&self, device: *mut c_void) -> *mut c_void {
        (self.device_get_queue)(device)
    }
}

/// A fully wired instance/surface/adapter/device chain for one native view.
#[derive(Debug, Clone, Copy)]
pub struct WgpuContext {
    /// The native view pointer this context renders into.
    pub view_ptr: *mut c_void,
    /// The wgpu instance.
    pub instance_ptr: *mut c_void,
    /// The wgpu surface.
    pub surface_ptr: *mut c_void,
    /// The wgpu adapter.
    pub adapter_ptr: *mut c_void,
    /// The wgpu device.
    pub device_ptr: *mut c_void,
}

impl WgpuContext {
    /// Build a context for an existing native view pointer. Mirrors Zig
    /// `WgpuContext.createForView`.
    pub fn create_for_view(
        core: &Core,
        native: &WgpuNative,
        view_ptr: *mut c_void,
    ) -> Result<WgpuContext> {
        let instance_ptr = native.create_instance();
        if instance_ptr.is_null() {
            return Err(ElectrobunError::Core(
                "wgpu instance creation failed".to_string(),
            ));
        }

        let surface_ptr = core.wgpu_create_surface_for_view(instance_ptr, view_ptr)?;

        // Core writes the adapter into slot 0 and the device into slot 1 of a
        // `[usize; 2]`, cast to `*mut c_void`. Preserve that layout exactly.
        let mut adapter_device: [usize; 2] = [0, 0];
        core.wgpu_create_adapter_device_main_thread(
            instance_ptr,
            surface_ptr,
            adapter_device.as_mut_ptr().cast::<c_void>(),
        )?;

        let adapter_ptr = adapter_device[0] as *mut c_void;
        let device_ptr = adapter_device[1] as *mut c_void;
        if device_ptr.is_null() {
            return Err(ElectrobunError::Core(
                "wgpu device creation failed".to_string(),
            ));
        }

        Ok(WgpuContext {
            view_ptr,
            instance_ptr,
            surface_ptr,
            adapter_ptr,
            device_ptr,
        })
    }

    /// Build a context for a WGPU view id by first resolving its native view
    /// pointer. Mirrors Zig `WgpuContext.createForWgpuView`.
    pub fn create_for_wgpu_view(
        core: &Core,
        native: &WgpuNative,
        wgpu_view_id: u32,
    ) -> Result<WgpuContext> {
        let view_ptr = core.get_wgpu_view_pointer(wgpu_view_id)?;
        Self::create_for_view(core, native, view_ptr)
    }

    /// Get the device's default queue.
    pub fn get_queue(&self, native: &WgpuNative) -> *mut c_void {
        native.device_get_queue(self.device_ptr)
    }
}
