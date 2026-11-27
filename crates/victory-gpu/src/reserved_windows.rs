//! Reserved row-major buffers backed by append-only 64 KiB tiles.
//!
//! Mapping does not initialize a tile. The caller must upload the returned
//! ranges before any shader can read them. This module never reuses a heap
//! slot, even after a backing is dropped.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use windows::Win32::Foundation::E_INVALIDARG;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC};

const TILE: usize = 64 << 10;
const HEAP_TILES: u32 = 256; // 16 MiB
const HEAP_BYTES: u64 = HEAP_TILES as u64 * TILE as u64;

/// First requester; timeline totals are cumulative mappings, not live bytes.
#[derive(Clone, Copy, Debug)]
pub(crate) enum RequestSource {
    SizedLookup,
    RootOrUnsized,
    ShaderAddress,
    DescriptorSrv,
    DescriptorUav,
    DescriptorCbv,
    Refresh,
    Dirty,
    GpuWrite,
    RootCbv,
}
#[cfg(feature = "diagnostics")]
static MAPPED_BY_SOURCE: [AtomicU64; 10] = [const { AtomicU64::new(0) }; 10];
#[cfg(feature = "diagnostics")]
pub(crate) fn mapped_by_source() -> [(RequestSource, u64); 10] {
    use RequestSource::*;
    [
        SizedLookup,
        RootOrUnsized,
        ShaderAddress,
        DescriptorSrv,
        DescriptorUav,
        DescriptorCbv,
        Refresh,
        Dirty,
        GpuWrite,
        RootCbv,
    ]
    .map(|s| (s, MAPPED_BY_SOURCE[s as usize].load(Ordering::Relaxed)))
}

#[derive(Default)]
struct Stats {
    mapped: AtomicU64,
    heaps: AtomicU64,
}

struct Heap {
    resource: ID3D12Heap,
    stats: Arc<Stats>,
}

impl Drop for Heap {
    fn drop(&mut self) {
        self.stats.heaps.fetch_sub(HEAP_BYTES, Ordering::Relaxed);
    }
}

#[derive(Default)]
struct State {
    current: Weak<Heap>,
    next: u32,
}

/// One pool per host. Allocation is serialized; mappings use the caller's
/// direct queue. A weak reference avoids retaining an otherwise unused heap.
#[derive(Default)]
pub(crate) struct Pool {
    state: Mutex<State>,
    stats: Arc<Stats>,
}

impl Pool {
    #[cfg(any(feature = "diagnostics", test))]
    pub(crate) fn mapped_bytes(&self) -> u64 {
        self.stats.mapped.load(Ordering::Relaxed)
    }

    #[cfg(any(feature = "diagnostics", test))]
    pub(crate) fn heap_bytes(&self) -> u64 {
        self.stats.heaps.load(Ordering::Relaxed)
    }

    /// Reserve an append-only run from one heap, making a new heap if needed.
    fn allocate(
        &self,
        device: &ID3D12Device,
        wanted: u32,
    ) -> windows::core::Result<(Arc<Heap>, u32, u32)> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let current = state.current.upgrade().filter(|_| state.next < HEAP_TILES);
        let heap = if let Some(heap) = current {
            heap
        } else {
            let desc = D3D12_HEAP_DESC {
                SizeInBytes: HEAP_BYTES,
                Properties: D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_DEFAULT,
                    ..Default::default()
                },
                Alignment: 0,
                Flags: D3D12_HEAP_FLAG_ALLOW_ONLY_BUFFERS,
            };
            let mut resource: Option<ID3D12Heap> = None;
            // SAFETY: the descriptor and output pointer are valid.
            unsafe { device.CreateHeap::<ID3D12Heap>(&desc, &mut resource)? };
            let resource = resource.ok_or_else(|| windows::core::Error::from(E_INVALIDARG))?;
            self.stats.heaps.fetch_add(HEAP_BYTES, Ordering::Relaxed);
            let heap = Arc::new(Heap {
                resource,
                stats: self.stats.clone(),
            });
            state.current = Arc::downgrade(&heap);
            state.next = 0;
            heap
        };
        let start = state.next;
        let count = wanted.min(HEAP_TILES - start);
        state.next += count;
        Ok((heap, start, count))
    }
}

/// Per-buffer mapping state. Heap references keep mapped physical memory
/// alive until the backing is dropped after GPU use has completed.
pub(crate) struct Backing {
    width: usize,
    tiles: Vec<Tile>,
    #[cfg(feature = "diagnostics")]
    sources: Option<Vec<Option<RequestSource>>>,
    requested: bool,
    heaps: Vec<Arc<Heap>>,
    stats: Option<Arc<Stats>>,
    mapped_count: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tile {
    Unmapped,
    Requested,
    Mapped,
}

impl Drop for Backing {
    fn drop(&mut self) {
        if let Some(stats) = &self.stats {
            stats
                .mapped
                .fetch_sub(self.mapped_count * TILE as u64, Ordering::Relaxed);
        }
    }
}

/// True only when unmapped tiles read as zero and discard writes.
pub(crate) fn supported(device: &ID3D12Device) -> bool {
    let mut options = D3D12_FEATURE_DATA_D3D12_OPTIONS::default();
    // SAFETY: the feature structure and size match.
    unsafe {
        device.CheckFeatureSupport(
            D3D12_FEATURE_D3D12_OPTIONS,
            (&raw mut options).cast(),
            size_of_val(&options) as u32,
        )
    }
    .is_ok()
        && options.TiledResourcesTier.0 >= 2
}

impl Backing {
    /// Create a tile-aligned reserved UAV-capable buffer in COMMON state.
    pub(crate) fn new(
        device: &ID3D12Device,
        width: u64,
    ) -> windows::core::Result<(ID3D12Resource, Self)> {
        let width = usize::try_from(width).map_err(|_| windows::core::Error::from(E_INVALIDARG))?;
        if width == 0 || !width.is_multiple_of(TILE) || width / TILE > u32::MAX as usize {
            return Err(windows::core::Error::from(E_INVALIDARG));
        }
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: width as u64,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_UNKNOWN,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            ..Default::default()
        };
        let mut resource: Option<ID3D12Resource> = None;
        // SAFETY: descriptor describes a buffer; no clear value applies.
        unsafe {
            device.CreateReservedResource(
                &desc,
                D3D12_RESOURCE_STATE_COMMON,
                None,
                &mut resource,
            )?
        };
        let resource = resource.ok_or_else(|| windows::core::Error::from(E_INVALIDARG))?;
        Ok((
            resource,
            Self {
                width,
                tiles: vec![Tile::Unmapped; width / TILE],
                #[cfg(feature = "diagnostics")]
                sources: crate::copystat::memory_timeline_enabled()
                    .then(|| vec![None; width / TILE]),
                requested: false,
                heaps: Vec::new(),
                stats: None,
                mapped_count: 0,
            },
        ))
    }

    /// Record the footprint in CPU state. Only submission preparation
    /// touches the queue or publishes physical mappings.
    pub(crate) fn request(&mut self, from: usize, to: usize, source: RequestSource) -> bool {
        #[cfg(not(feature = "diagnostics"))]
        let _ = source;
        debug_assert!(from <= to && to <= self.width);
        if from >= to || self.mapped_count as usize == self.tiles.len() {
            return false;
        }
        for (index, tile) in self
            .tiles
            .iter_mut()
            .enumerate()
            .take(to.div_ceil(TILE))
            .skip(from / TILE)
        {
            #[cfg(not(feature = "diagnostics"))]
            let _ = index;
            if *tile == Tile::Unmapped {
                *tile = Tile::Requested;
                #[cfg(feature = "diagnostics")]
                if let Some(sources) = &mut self.sources {
                    sources[index] = Some(source);
                }
                self.requested = true;
            }
        }
        self.requested
    }

    /// Map every requested tile. Ranges returned are
    /// full tiles and must be initialized before the buffer is read. Queue
    /// ordering keeps mappings ahead of subsequent uploads on that queue.
    /// Callers must treat a device-removal error as terminal for this backing.
    pub(crate) fn ensure(
        &mut self,
        pool: &Pool,
        device: &ID3D12Device,
        queue: &ID3D12CommandQueue,
        resource: &ID3D12Resource,
    ) -> windows::core::Result<Vec<(usize, usize)>> {
        if !self.requested {
            return Ok(Vec::new());
        }
        debug_assert!(
            self.stats
                .as_ref()
                .is_none_or(|s| Arc::ptr_eq(s, &pool.stats))
        );
        let runs = requested_runs(&self.tiles);
        // Reserve every heap run before queuing any mapping. A failed heap
        // allocation therefore cannot hide earlier newly mapped ranges.
        let mut allocations = Vec::new();
        for &(first, end) in &runs {
            let mut at = first;
            while at < end {
                let (heap, offset, count) =
                    pool.allocate(device, (end - at).min(u32::MAX as usize) as u32)?;
                allocations.push((at, count, heap, offset));
                at += count as usize;
            }
        }
        if !allocations.is_empty() {
            self.stats = Some(pool.stats.clone());
        }
        for (first, count, heap, offset) in allocations {
            let coord = D3D12_TILED_RESOURCE_COORDINATE {
                X: first as u32,
                ..Default::default()
            };
            let region = D3D12_TILE_REGION_SIZE {
                NumTiles: count,
                ..Default::default()
            };
            let flag = D3D12_TILE_RANGE_FLAG_NONE;
            // SAFETY: logical tiles and append-only heap run are in bounds;
            // the queue orders this mapping before the caller's later work.
            unsafe {
                queue.UpdateTileMappings(
                    resource,
                    1,
                    Some(&coord),
                    Some(&region),
                    &heap.resource,
                    1,
                    Some(&flag),
                    Some(&offset),
                    Some(&count),
                    D3D12_TILE_MAPPING_FLAG_NONE,
                );
            }
            self.tiles[first..first + count as usize].fill(Tile::Mapped);
            self.mapped_count += u64::from(count);
            pool.stats
                .mapped
                .fetch_add(u64::from(count) * TILE as u64, Ordering::Relaxed);
            self.heaps.push(heap);
            // A removed device is terminal: mappings cannot safely be retried.
            unsafe { device.GetDeviceRemovedReason()? };
            #[cfg(feature = "diagnostics")]
            if let Some(sources) = &mut self.sources {
                for source in &mut sources[first..first + count as usize] {
                    if let Some(source) = source.take() {
                        MAPPED_BY_SOURCE[source as usize].fetch_add(TILE as u64, Ordering::Relaxed);
                    }
                }
            }
        }
        self.requested = false;
        Ok(runs
            .into_iter()
            .map(|(a, b)| (a * TILE, b * TILE))
            .collect())
    }
}

fn requested_runs(tiles: &[Tile]) -> Vec<(usize, usize)> {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for (i, _) in tiles
        .iter()
        .enumerate()
        .filter(|(_, tile)| **tile == Tile::Requested)
    {
        match runs.last_mut() {
            Some((_, end)) if *end == i => *end += 1,
            _ => runs.push((i, i + 1)),
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::{Tile::*, requested_runs};

    #[cfg(feature = "diagnostics")]
    #[test]
    fn first_requester_survives_overlap_and_mapped_tiles() {
        use super::{Backing, RequestSource, TILE};
        let mut backing = Backing {
            width: TILE * 3,
            tiles: vec![Unmapped, Unmapped, Mapped],
            sources: Some(vec![None; 3]),
            requested: false,
            heaps: Vec::new(),
            stats: None,
            mapped_count: 1,
        };
        backing.request(1, TILE + 1, RequestSource::SizedLookup);
        backing.request(TILE, TILE * 3, RequestSource::GpuWrite);
        let sources = backing.sources.as_ref().unwrap();
        assert!(matches!(sources[0], Some(RequestSource::SizedLookup)));
        assert!(matches!(sources[1], Some(RequestSource::SizedLookup)));
        assert!(sources[2].is_none());
    }

    #[test]
    fn requested_runs_skip_existing_tiles_and_join_neighbors() {
        assert_eq!(
            requested_runs(&[Requested, Mapped, Requested, Requested, Unmapped, Requested]),
            [(0, 1), (2, 4), (5, 6)]
        );
        assert_eq!(requested_runs(&[Unmapped, Mapped, Unmapped]), []);
    }
}
