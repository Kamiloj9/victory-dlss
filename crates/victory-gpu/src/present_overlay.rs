//! Synchronous fullscreen premultiplied-alpha present overlay.
//!
//! The caller supplies compatible formats and resource states, serializes use,
//! and waits for GPU completion before reusing or dropping this pass.
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_SAMPLE_DESC};

const SOURCE: &str = r#"
Texture2D<float4> plane : register(t0);
SamplerState bilinear : register(s0);
struct Corner { float4 position : SV_Position; float2 at : TEXCOORD0; };
Corner corner(uint i : SV_VertexID) {
    Corner c;
    c.at = float2(float(i & 1) * 2.0, float(i >> 1) * 2.0);
    c.position = float4(c.at.x * 2.0 - 1.0, 1.0 - c.at.y * 2.0, 0.0, 1.0);
    return c;
}
float4 over(Corner c) : SV_Target { return plane.Sample(bilinear, c.at); }
"#;

pub(crate) struct PresentOverlay {
    format: DXGI_FORMAT,
    root: ID3D12RootSignature,
    pipeline: ID3D12PipelineState,
    targets: ID3D12DescriptorHeap,
    views: ID3D12DescriptorHeap,
}

/// A bilinear, clamping sampler at `s0` for the stages `visibility` names.
fn bilinear(visibility: D3D12_SHADER_VISIBILITY) -> D3D12_STATIC_SAMPLER_DESC {
    D3D12_STATIC_SAMPLER_DESC {
        Filter: D3D12_FILTER_MIN_MAG_LINEAR_MIP_POINT,
        AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        ComparisonFunc: D3D12_COMPARISON_FUNC_NEVER,
        MaxLOD: f32::MAX,
        ShaderRegister: 0,
        RegisterSpace: 0,
        ShaderVisibility: visibility,
        ..Default::default()
    }
}

/// A root signature of `parameters` and one bilinear sampler, serialized by
/// the runtime and not taken from a shader: the system compiler under Wine
/// compiles a `[RootSignature]` attribute without writing the signature
/// into the shader, and `CreateRootSignature` then fails on it (#188).
fn root_signature(
    device: &ID3D12Device,
    parameters: &[D3D12_ROOT_PARAMETER],
    sampler: D3D12_SHADER_VISIBILITY,
    flags: D3D12_ROOT_SIGNATURE_FLAGS,
) -> windows::core::Result<ID3D12RootSignature> {
    use windows::Win32::Graphics::Direct3D::ID3DBlob;
    let samplers = [bilinear(sampler)];
    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: parameters.len() as u32,
        pParameters: parameters.as_ptr(),
        NumStaticSamplers: 1,
        pStaticSamplers: samplers.as_ptr(),
        Flags: flags,
    };
    let mut blob: Option<ID3DBlob> = None;
    // SAFETY: the description points at live slices; the blob is the
    // runtime's own, read while it lives.
    unsafe {
        D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &mut blob, None)?;
        let blob = blob.ok_or_else(windows::core::Error::empty)?;
        let bytes =
            std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize());
        device.CreateRootSignature(0, bytes)
    }
}

/// One descriptor range of one view at register 0, at `offset` in its table.
fn range(kind: D3D12_DESCRIPTOR_RANGE_TYPE, offset: u32) -> D3D12_DESCRIPTOR_RANGE {
    D3D12_DESCRIPTOR_RANGE {
        RangeType: kind,
        NumDescriptors: 1,
        BaseShaderRegister: 0,
        RegisterSpace: 0,
        OffsetInDescriptorsFromTableStart: offset,
    }
}

/// A root parameter that is a table of `ranges`, for the stages `visibility`
/// names.
fn table(
    ranges: &[D3D12_DESCRIPTOR_RANGE],
    visibility: D3D12_SHADER_VISIBILITY,
) -> D3D12_ROOT_PARAMETER {
    D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
        Anonymous: D3D12_ROOT_PARAMETER_0 {
            DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                NumDescriptorRanges: ranges.len() as u32,
                pDescriptorRanges: ranges.as_ptr(),
            },
        },
        ShaderVisibility: visibility,
    }
}

impl PresentOverlay {
    pub(crate) fn new(device: &ID3D12Device, format: DXGI_FORMAT) -> windows::core::Result<Self> {
        use windows::core::s;
        let vs = crate::host::compile(SOURCE, s!("corner"), s!("vs_5_1"))?;
        let ps = crate::host::compile(SOURCE, s!("over"), s!("ps_5_1"))?;
        let code = |bytes: &[u8]| D3D12_SHADER_BYTECODE {
            pShaderBytecode: bytes.as_ptr().cast(),
            BytecodeLength: bytes.len(),
        };
        let plane = [range(D3D12_DESCRIPTOR_RANGE_TYPE_SRV, 0)];
        let root = root_signature(
            device,
            &[table(&plane, D3D12_SHADER_VISIBILITY_PIXEL)],
            D3D12_SHADER_VISIBILITY_PIXEL,
            D3D12_ROOT_SIGNATURE_FLAG_NONE,
        )?;
        // SAFETY: the description points at live data and never releases its
        // pointer.
        unsafe {
            let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
                pRootSignature: std::mem::transmute_copy(&root),
                VS: code(&vs[..]),
                PS: code(&ps[..]),
                SampleMask: u32::MAX,
                RasterizerState: D3D12_RASTERIZER_DESC {
                    FillMode: D3D12_FILL_MODE_SOLID,
                    CullMode: D3D12_CULL_MODE_NONE,
                    DepthClipEnable: true.into(),
                    ..Default::default()
                },
                PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
                NumRenderTargets: 1,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                ..Default::default()
            };
            desc.RTVFormats[0] = format;
            desc.BlendState.RenderTarget[0] = D3D12_RENDER_TARGET_BLEND_DESC {
                BlendEnable: true.into(),
                SrcBlend: D3D12_BLEND_ONE,
                DestBlend: D3D12_BLEND_INV_SRC_ALPHA,
                BlendOp: D3D12_BLEND_OP_ADD,
                SrcBlendAlpha: D3D12_BLEND_ONE,
                DestBlendAlpha: D3D12_BLEND_INV_SRC_ALPHA,
                BlendOpAlpha: D3D12_BLEND_OP_ADD,
                RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
                ..Default::default()
            };
            let pipeline: ID3D12PipelineState = device.CreateGraphicsPipelineState(&desc)?;
            let targets: ID3D12DescriptorHeap =
                device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
                    NumDescriptors: 1,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
                    NodeMask: 0,
                })?;
            let views: ID3D12DescriptorHeap =
                device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                    NumDescriptors: 1,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                    NodeMask: 0,
                })?;
            Ok(Self {
                format,
                root,
                pipeline,
                targets,
                views,
            })
        }
    }

    /// Records one full-window overlay; resource barriers are the caller's responsibility.
    pub(crate) fn record(
        &self,
        device: &ID3D12Device,
        list: &ID3D12GraphicsCommandList,
        back: &ID3D12Resource,
        (width, height): (u32, u32),
        plane: &ID3D12Resource,
        plane_format: DXGI_FORMAT,
    ) {
        let view = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: plane_format,
            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2D: D3D12_TEX2D_SRV {
                    MostDetailedMip: 0,
                    MipLevels: 1,
                    ..Default::default()
                },
            },
        };
        let target_desc = D3D12_RENDER_TARGET_VIEW_DESC {
            Format: self.format,
            ViewDimension: D3D12_RTV_DIMENSION_TEXTURE2D,
            ..Default::default()
        };
        // SAFETY: an open list; slots of our heaps; live resources and objects
        // (the pass's own live as long as the process or until remade, and a
        // list holds what it was recorded with).
        unsafe {
            let target = self.targets.GetCPUDescriptorHandleForHeapStart();
            let cpu = self.views.GetCPUDescriptorHandleForHeapStart();
            let gpu = self.views.GetGPUDescriptorHandleForHeapStart();
            device.CreateRenderTargetView(back, Some(&target_desc), target);
            device.CreateShaderResourceView(plane, Some(&view), cpu);
            list.SetDescriptorHeaps(&[Some(self.views.clone())]);
            list.SetGraphicsRootSignature(&self.root);
            list.SetPipelineState(&self.pipeline);
            list.SetGraphicsRootDescriptorTable(0, gpu);
            list.OMSetRenderTargets(1, Some(&target), false, None);
            list.RSSetViewports(&[D3D12_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: width as f32,
                Height: height as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]);
            list.RSSetScissorRects(&[RECT {
                left: 0,
                top: 0,
                right: width as i32,
                bottom: height as i32,
            }]);
            list.IASetPrimitiveTopology(
                windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY(4),
            );
            list.DrawInstanced(3, 1, 0, 0);
        }
    }

    // ---------------------------------------------------------------------------
    // The upscaler's output, by a plain stretch
    // ---------------------------------------------------------------------------
}

#[cfg(all(test, feature = "diagnostics"))]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

    /// Run alone in a diagnostics build with WARP, the debug layer and GBV enabled.
    #[test]
    #[ignore = "requires WARP, the D3D12 debug layer and GPU-based validation"]
    fn warp_premultiplied_overlay_readback() {
        assert!(std::env::var_os("VICTORY_D3D12_WARP").is_some());
        assert!(std::env::var_os("VICTORY_D3D12_DEBUG").is_some());
        assert!(std::env::var_os("VICTORY_D3D12_GBV").is_some());
        let host = crate::host::host().expect("D3D12 host");
        let format = DXGI_FORMAT_R8G8B8A8_UNORM;
        let make_texture = |width, height| {
            let heap = D3D12_HEAP_PROPERTIES {
                Type: D3D12_HEAP_TYPE_DEFAULT,
                ..Default::default()
            };
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Width: width,
                Height: height,
                DepthOrArraySize: 1,
                MipLevels: 1,
                Format: format,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
                ..Default::default()
            };
            let mut texture = None;
            // SAFETY: valid descriptions and an output pointer.
            unsafe {
                host.device
                    .CreateCommittedResource(
                        &heap,
                        D3D12_HEAP_FLAG_NONE,
                        &desc,
                        D3D12_RESOURCE_STATE_RENDER_TARGET,
                        None,
                        &mut texture,
                    )
                    .unwrap()
            };
            texture.unwrap()
        };
        let base = make_texture(4, 4);
        let plane = make_texture(2, 2);
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_READBACK,
            ..Default::default()
        };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: 1024,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            ..Default::default()
        };
        let mut out = None;
        // SAFETY: valid readback buffer description.
        unsafe {
            host.device
                .CreateCommittedResource(
                    &heap,
                    D3D12_HEAP_FLAG_NONE,
                    &desc,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    None,
                    &mut out,
                )
                .unwrap()
        };
        let readback: ID3D12Resource = out.unwrap();
        let overlay = PresentOverlay::new(&host.device, format).unwrap();
        let list = host.new_list().unwrap();
        let base_rtv = host.rtv(Some(&base), None);
        let plane_rtv = host.rtv(Some(&plane), None);
        // SAFETY: both resources are render targets and the command list is open.
        unsafe {
            list.list
                .ClearRenderTargetView(base_rtv, &[0.0, 0.0, 1.0, 1.0], None);
            list.list
                .ClearRenderTargetView(plane_rtv, &[0.5, 0.0, 0.0, 0.5], None);
            list.list.ResourceBarrier(&[crate::host::transition(
                &plane,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
                D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
            )]);
        }
        overlay.record(&host.device, &list.list, &base, (4, 4), &plane, format);
        // SAFETY: the overlay writes `base`; transition before copying to readback.
        unsafe {
            list.list.ResourceBarrier(&[crate::host::transition(
                &base,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            )]);
            let dst = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(&readback),
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                        Offset: 0,
                        Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                            Format: format,
                            Width: 4,
                            Height: 4,
                            Depth: 1,
                            RowPitch: 256,
                        },
                    },
                },
            };
            let src = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(&base),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    SubresourceIndex: 0,
                },
            };
            list.list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
        }
        list.close();
        host.execute(&[&list]);
        let mut ptr = std::ptr::null_mut();
        // SAFETY: execute waits; the readback has four rows of 256 bytes.
        unsafe {
            readback.Map(0, None, Some(&mut ptr)).unwrap();
            for y in 0..4 {
                for x in 0..4 {
                    let pixel =
                        std::slice::from_raw_parts(ptr.cast::<u8>().add(y * 256 + x * 4), 4);
                    for (actual, expected) in pixel.iter().zip([128i16, 0, 127, 255]) {
                        assert!(
                            (i16::from(*actual) - expected).abs() <= 2,
                            "pixel {x},{y}: {pixel:?}"
                        );
                    }
                }
            }
            readback.Unmap(0, None);
        }
        assert_eq!(
            host.log_messages(),
            None,
            "D3D12 debug layer reported an error"
        );
    }
}
