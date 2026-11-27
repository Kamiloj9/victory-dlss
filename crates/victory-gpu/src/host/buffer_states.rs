use windows::Win32::Graphics::Direct3D12::*;

/// Every read of a host copy of title memory (vertex, index, constant and
/// shader-resource data, indirect counts, the argument clamp's sources).
pub(crate) const BUFFER_READ: u32 = D3D12_RESOURCE_STATE_GENERIC_READ.0 as u32;
/// Shader writes to a host copy of title memory.
pub(crate) const BUFFER_WRITE: u32 = D3D12_RESOURCE_STATE_UNORDERED_ACCESS.0 as u32;

/// The states of the host copies of title memory (buffers) one list uses,
/// by copy: the state it starts in and the one it is in now (upstream
/// VIctory #334). A buffer is in the common state between lists: the
/// list's prelude moves it to its first state and the list moves it back
/// before it closes. A buffer a shader writes and a later command reads
/// (the meshlet cull's arguments and counts, vertices) is transitioned in
/// between; the implicit promotion from common it relied on allows either
/// use in a list, not both.
///
/// Within one command (a draw, a dispatch, the argument clamp) the write
/// state wins: a copy one command both writes and reads (one host copy
/// backs most of the title's buffers) stays writable, whatever order the
/// command's views are bound in.
///
/// shortcut: buffers read only through directly indexed heaps are not
/// tracked, and a command that both writes a buffer and reads it as
/// vertices, indices or counts reads them in the write state.
pub(crate) struct BufferStates<R> {
    /// First state, current state, the command that last wrote it.
    by_copy: std::collections::HashMap<usize, (u32, u32, Option<u64>, R)>,
    command: u64,
}

impl<R> Default for BufferStates<R> {
    fn default() -> Self {
        Self {
            by_copy: std::collections::HashMap::new(),
            command: 0,
        }
    }
}

impl<R: Clone> BufferStates<R> {
    /// Uses from here on belong to the next command.
    pub(crate) fn next_command(&mut self) {
        self.command += 1;
    }

    /// The list uses `copy` (`resource`) in `wanted`: the state it leaves,
    /// if a transition must be recorded now. A first use records nothing:
    /// the prelude moves the buffer there.
    pub(crate) fn update(&mut self, copy: usize, resource: &R, wanted: u32) -> Option<u32> {
        let command = self.command;
        let wrote = (wanted == BUFFER_WRITE).then_some(command);
        match self.by_copy.entry(copy) {
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert((wanted, wanted, wrote, resource.clone()));
                None
            }
            std::collections::hash_map::Entry::Occupied(mut o) => {
                let e = o.get_mut();
                if wrote.is_some() {
                    e.2 = wrote;
                } else if e.2 == Some(command) {
                    return None;
                }
                let before = std::mem::replace(&mut e.1, wanted);
                (before != wanted).then_some(before)
            }
        }
    }

    /// The prelude's transitions: each buffer from common to its first state.
    pub(crate) fn firsts(&self) -> impl Iterator<Item = (&R, u32)> {
        self.by_copy.values().map(|(first, _, _, r)| (r, *first))
    }

    /// The list's closing transitions: each buffer from its last state back
    /// to common.
    pub(crate) fn lasts(&self) -> impl Iterator<Item = (&R, u32)> {
        self.by_copy.values().map(|(_, last, _, r)| (r, *last))
    }

    pub(crate) fn clear(&mut self) {
        self.by_copy.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;

    #[test]
    fn buffer_states_transition_between_writes_and_reads() {
        let mut s = BufferStates::default();
        // First uses: the prelude's.
        assert_eq!(s.update(1, &'a', BUFFER_WRITE), None);
        assert_eq!(s.update(2, &'b', BUFFER_READ), None);
        assert_eq!(s.update(1, &'a', BUFFER_WRITE), None);
        s.next_command();
        assert_eq!(s.update(1, &'a', BUFFER_READ), Some(BUFFER_WRITE));
        assert_eq!(s.update(1, &'a', BUFFER_READ), None);
        s.next_command();
        assert_eq!(s.update(1, &'a', BUFFER_WRITE), Some(BUFFER_READ));
        let sorted = |mut v: Vec<(char, u32)>| {
            v.sort_unstable();
            v
        };
        let firsts = sorted(s.firsts().map(|(r, f)| (*r, f)).collect());
        assert_eq!(firsts, [('a', BUFFER_WRITE), ('b', BUFFER_READ)]);
        let lasts = sorted(s.lasts().map(|(r, l)| (*r, l)).collect());
        assert_eq!(lasts, [('a', BUFFER_WRITE), ('b', BUFFER_READ)]);
        s.clear();
        assert_eq!(s.firsts().count(), 0);
    }

    /// One command that writes a copy and reads it: the write state wins
    /// whichever is bound first; the next command reads again.
    #[test]
    fn buffer_states_keep_a_command_writing() {
        let mut s = BufferStates::default();
        s.next_command();
        assert_eq!(s.update(1, &'a', BUFFER_WRITE), None);
        assert_eq!(s.update(1, &'a', BUFFER_READ), None);
        s.next_command();
        assert_eq!(s.update(1, &'a', BUFFER_READ), Some(BUFFER_WRITE));
        assert_eq!(s.update(1, &'a', BUFFER_WRITE), Some(BUFFER_READ));
        assert_eq!(s.update(1, &'a', BUFFER_READ), None);
        s.next_command();
        assert_eq!(s.update(1, &'a', BUFFER_READ), Some(BUFFER_WRITE));
        let lasts: Vec<_> = s.lasts().map(|(r, l)| (*r, l)).collect();
        assert_eq!(lasts, [('a', BUFFER_READ)]);
    }

    #[cfg(feature = "diagnostics")]
    #[test]
    #[ignore = "requires WARP and D3D12 GPU-based validation"]
    fn warp_shadow_buffers_switch_between_shader_write_and_read() {
        assert!(std::env::var_os("VICTORY_D3D12_GBV").is_some());
        TEST_BUFFER_STATE_VALIDATION.store(true, Ordering::Relaxed);
        let host = host().expect("WARP host");
        let adapter = host.adapter.as_ref().expect("WARP adapter");
        // SAFETY: this is a query on the live adapter.
        let adapter = unsafe { adapter.GetDesc1().expect("adapter description") };
        assert_ne!(adapter.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32, 0);
        assert!(host.messages.is_some(), "D3D12 validation must be enabled");
        let shader = r#"
#define RS "RootConstants(num32BitConstants=1, b0), DescriptorTable(SRV(t0), UAV(u0))"
cbuffer Data : register(b0) { uint key; };
Buffer<float> input : register(t0);
RWBuffer<float> output : register(u0);
[RootSignature(RS)]
[numthreads(1, 1, 1)]
void main() { output[0] = asfloat(asuint(input.Load(0)) ^ key); }
"#;
        let code = compile(
            shader,
            windows::core::s!("main"),
            windows::core::s!("cs_5_1"),
        )
        .expect("transfer shader");
        // SAFETY: the shader carries its root signature.
        let (root, pipeline) = unsafe {
            let root: ID3D12RootSignature = host.device.CreateRootSignature(0, &code).unwrap();
            let pipeline: ID3D12PipelineState = host
                .device
                .CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                    pRootSignature: std::mem::transmute_copy(&root),
                    CS: D3D12_SHADER_BYTECODE {
                        pShaderBytecode: code.as_ptr().cast(),
                        BytecodeLength: code.len(),
                    },
                    ..Default::default()
                })
                .unwrap();
            (root, pipeline)
        };
        let (a, a_data) = host.shared_buffer(4).expect("first title shadow");
        let (b, b_data) = host.shared_buffer(4).expect("second title shadow");
        // A float view must roundtrip packed-coordinate bits, including subnormals.
        let srv = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: DXGI_FORMAT_R32_FLOAT,
            ViewDimension: D3D12_SRV_DIMENSION_BUFFER,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                Buffer: D3D12_BUFFER_SRV {
                    NumElements: 1,
                    ..Default::default()
                },
            },
        };
        let uav = D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: DXGI_FORMAT_R32_FLOAT,
            ViewDimension: D3D12_UAV_DIMENSION_BUFFER,
            Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
                Buffer: D3D12_BUFFER_UAV {
                    NumElements: 1,
                    ..Default::default()
                },
            },
        };
        // SAFETY: four descriptors of valid one-word buffers.
        let (heap, first, step) = unsafe {
            let kind = D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV;
            let heap: ID3D12DescriptorHeap = host
                .device
                .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: kind,
                    NumDescriptors: 4,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                    NodeMask: 0,
                })
                .unwrap();
            let base = heap.GetCPUDescriptorHandleForHeapStart().ptr;
            let step = host.device.GetDescriptorHandleIncrementSize(kind);
            let slot = |i| D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: base + i * step as usize,
            };
            for (i, (source, target)) in [(&a, &b), (&b, &a)].into_iter().enumerate() {
                host.device
                    .CreateShaderResourceView(source, Some(&srv), slot(2 * i));
                host.device
                    .CreateUnorderedAccessView(target, None, Some(&uav), slot(2 * i + 1));
            }
            let first = heap.GetGPUDescriptorHandleForHeapStart();
            (heap, first, step)
        };
        let heaps = [Some(heap)];
        let dispatch = |list: &HostList,
                        input: &ID3D12Resource,
                        output: &ID3D12Resource,
                        swap: bool,
                        key: u32| {
            list.begin_command();
            list.buffer_state(input, BUFFER_READ);
            list.buffer_state(output, BUFFER_WRITE);
            // SAFETY: the list is open, and both buffers hold one word.
            unsafe {
                list.list.SetPipelineState(&pipeline);
                list.list.SetComputeRootSignature(&root);
                list.list.SetComputeRoot32BitConstant(0, key, 0);
                list.list.SetDescriptorHeaps(&heaps);
                list.list.SetComputeRootDescriptorTable(
                    1,
                    D3D12_GPU_DESCRIPTOR_HANDLE {
                        ptr: first.ptr + 2 * u64::from(step) * u64::from(swap),
                    },
                );
                list.list.Dispatch(1, 1, 1);
            }
        };
        for split in [false, true] {
            const INITIAL: u32 = 0x0008_0007;
            const KEY1: u32 = 0x0010_0010;
            const KEY2: u32 = 0x8000_0000;
            // SAFETY: freshly mapped four-byte buffers. The previous submission finished.
            unsafe {
                a_data.cast::<u32>().write(INITIAL);
                b_data.cast::<u32>().write(0);
            }
            let producer = host.new_list().expect("producer list");
            dispatch(&producer, &a, &b, false, KEY1);
            if split {
                let consumer = host.new_list().expect("consumer list");
                dispatch(&consumer, &b, &a, true, KEY2);
                producer.close();
                consumer.close();
                host.execute(&[&producer, &consumer]);
            } else {
                dispatch(&producer, &b, &a, true, KEY2);
                producer.close();
                host.execute(&[&producer]);
            }
            host.log_messages();
            let validation_error = TEST_BUFFER_STATE_ERROR.lock().unwrap().take();
            assert_eq!(
                validation_error, None,
                "D3D12 resource-state validation, split={split}"
            );
            // SAFETY: the synchronous submission completed both shader writes.
            unsafe {
                assert_eq!(b_data.cast::<u32>().read(), INITIAL ^ KEY1);
                assert_eq!(a_data.cast::<u32>().read(), INITIAL ^ KEY1 ^ KEY2);
            }
        }
    }
}
