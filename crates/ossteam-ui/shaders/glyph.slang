// SPDX-License-Identifier: LGPL-2.1-or-later
// Glyphs from the atlas, drawn 1:1 at whole device pixels (scene::Glyph),
// so the atlas is read with Load and needs no sampler.

struct Push
{
    float2 viewport;
};

[[vk::push_constant]]
ConstantBuffer<Push> push;

[[vk::binding(0, 0)]]
Texture2D<float4> atlas;

struct Instance
{
    [[vk::location(0)]] float4 dest;  // x, y, width, height
    [[vk::location(1)]] float4 slot;  // x, y in the atlas; z = 1 for a colour glyph
    [[vk::location(2)]] float4 color; // straight alpha
    [[vk::location(3)]] float4 clip;  // x0, y0, x1, y1
};

struct Varyings
{
    float4 position : SV_Position;
    nointerpolation float4 dest : DEST;
    nointerpolation float4 slot : SLOT;
    nointerpolation float4 color : COLOR;
};

[shader("vertex")]
Varyings vertexMain(Instance glyph, uint vertex : SV_VulkanVertexID)
{
    float2 corner = float2(float(vertex & 1), float(vertex >> 1));
    float2 p0 = max(glyph.dest.xy, floor(glyph.clip.xy));
    float2 p1 = min(glyph.dest.xy + glyph.dest.zw, ceil(glyph.clip.zw));
    p1 = max(p1, p0);
    float2 pos = lerp(p0, p1, corner);

    Varyings out;
    out.position = float4(pos / push.viewport * 2.0 - 1.0, 0.0, 1.0);
    out.dest = glyph.dest;
    out.slot = glyph.slot;
    out.color = glyph.color;
    return out;
}

[shader("fragment")]
float4 fragmentMain(Varyings v) : SV_Target
{
    int2 texel = int2(v.slot.xy + floor(v.position.xy - v.dest.xy));
    float4 s = atlas.Load(int3(texel, 0));
    float a = s.a * v.color.a;
    if (v.slot.z > 0.5)
        return float4(s.rgb * a, a);
    return float4(v.color.rgb * a, a);
}
