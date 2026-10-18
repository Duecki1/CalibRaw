//! Vulkan (Linux, Android): `VK_EXT_memory_budget`, which wgpu enables on
//! every device whose driver has it.

use super::AdapterMemory;
use ash::{ext, khr, vk};
use wgpu::hal::api::Vulkan;

pub(super) fn adapter_memory(device: &wgpu::Device) -> Option<AdapterMemory> {
    // SAFETY: the HAL device is only read (its physical device, instance and
    // enabled extensions) and nothing is created, destroyed or submitted
    // through it. The guard keeps it alive until the end of this function.
    let hal_device = unsafe { device.as_hal::<Vulkan>() }?;
    if !hal_device
        .enabled_device_extensions()
        .contains(&ext::memory_budget::NAME)
    {
        return None;
    }
    let instance = hal_device.shared_instance();
    // wgpu enables this instance extension wherever the loader has it, and
    // loads its own copy of these functions the same way.
    if !instance
        .extensions()
        .contains(&khr::get_physical_device_properties2::NAME)
    {
        return None;
    }
    let properties2 = khr::get_physical_device_properties2::Instance::new(
        instance.entry(),
        instance.raw_instance(),
    );
    let mut budget = vk::PhysicalDeviceMemoryBudgetPropertiesEXT::default();
    let mut properties = vk::PhysicalDeviceMemoryProperties2::default().push_next(&mut budget);
    // SAFETY: the physical device belongs to `instance`, which the HAL device
    // keeps alive. The device has VK_EXT_memory_budget, so its physical device
    // supports the chained struct, and both structs outlive the call. Physical
    // device queries need no external synchronization.
    unsafe {
        properties2.get_physical_device_memory_properties2(
            hal_device.raw_physical_device(),
            &mut properties,
        );
    }
    let memory = properties.memory_properties;
    let heap_count = (memory.memory_heap_count as usize).min(vk::MAX_MEMORY_HEAPS);
    device_local_memory(
        &memory.memory_heaps[..heap_count],
        &budget.heap_budget,
        &budget.heap_usage,
    )
}

/// The budget and usage of all device-local heaps together, or `None` if
/// there is none. Discrete GPUs may split video memory into several such
/// heaps (AMD's CPU-visible 256 MiB window); system memory heaps are left out,
/// as DXGI's local segment group leaves them out.
fn device_local_memory(
    heaps: &[vk::MemoryHeap],
    budgets: &[u64],
    usages: &[u64],
) -> Option<AdapterMemory> {
    heaps
        .iter()
        .zip(budgets.iter().zip(usages))
        .filter(|(heap, _)| heap.flags.contains(vk::MemoryHeapFlags::DEVICE_LOCAL))
        .map(|(_, (&budget, &usage))| AdapterMemory { budget, usage })
        .reduce(|total, heap| AdapterMemory {
            budget: total.budget.saturating_add(heap.budget),
            usage: total.usage.saturating_add(heap.usage),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1 << 20;

    fn heap(size: u64, flags: vk::MemoryHeapFlags) -> vk::MemoryHeap {
        vk::MemoryHeap { size, flags }
    }

    #[test]
    fn device_local_heaps_are_summed_and_system_memory_is_left_out() {
        let local = vk::MemoryHeapFlags::DEVICE_LOCAL;
        let system = vk::MemoryHeapFlags::empty();

        // Discrete GPU: video memory, then system memory.
        assert_eq!(
            device_local_memory(
                &[heap(8192 * MIB, local), heap(16384 * MIB, system)],
                &[7900 * MIB, 8000 * MIB],
                &[1200 * MIB, 64 * MIB],
            ),
            Some(AdapterMemory {
                budget: 7900 * MIB,
                usage: 1200 * MIB,
            })
        );

        // AMD without resizable BAR: video memory split around the 256 MiB
        // CPU-visible window.
        assert_eq!(
            device_local_memory(
                &[
                    heap(7936 * MIB, local),
                    heap(16384 * MIB, system),
                    heap(256 * MIB, local),
                ],
                &[7600 * MIB, 8000 * MIB, 250 * MIB],
                &[900 * MIB, 64 * MIB, 40 * MIB],
            ),
            Some(AdapterMemory {
                budget: 7850 * MIB,
                usage: 940 * MIB,
            })
        );

        // Integrated or mobile GPU: one heap shared with the system.
        assert_eq!(
            device_local_memory(&[heap(6144 * MIB, local)], &[3000 * MIB], &[700 * MIB]),
            Some(AdapterMemory {
                budget: 3000 * MIB,
                usage: 700 * MIB,
            })
        );

        assert_eq!(
            device_local_memory(&[heap(8192 * MIB, system)], &[4000 * MIB], &[0]),
            None
        );
    }
}
