// SPDX-License-Identifier: LGPL-2.1-or-later
// Images drawn 1:1 at whole device pixels (scene::ImageQuad): each image
// is its own texture, read with Load, so there is no sampler. Texels are
// premultiplied RGBA. Corners are cut as quad.slang cuts a quad's, and
// as render/software.rs does; keep the three in step.

struct Push
{
    float2 viewport;
};

[[vk::push_constant]]
ConstantBuffer<Push> push;

[[vk::binding(0, 0)]]
Texture2D<float4> image;

struct Instance
{
    [[vk::location(0)]] float4 dest;   // x, y, width, height
    [[vk::location(1)]] float4 clip;   // x0, y0, x1, y1
    [[vk::location(2)]] float4 params; // x = opacity, y = corner radius
    [[vk::location(3)]] float4 round;  // x, y, width, height
};

struct Varyings
{
    float4 position : SV_Position;
    nointerpolation float4 dest : DEST;
    nointerpolation float opacity : OPACITY;
    nointerpolation float radius : RADIUS;
    nointerpolation float4 round : ROUND;
};

[shader("vertex")]
Varyings vertexMain(Instance quad, uint vertex : SV_VulkanVertexID)
{
    float2 corner = float2(float(vertex & 1), float(vertex >> 1));
    float2 p0 = max(quad.dest.xy, floor(quad.clip.xy));
    float2 p1 = min(quad.dest.xy + quad.dest.zw, ceil(quad.clip.zw));
    p1 = max(p1, p0);
    float2 pos = lerp(p0, p1, corner);

    Varyings out;
    out.position = float4(pos / push.viewport * 2.0 - 1.0, 0.0, 1.0);
    out.dest = quad.dest;
    out.opacity = quad.params.x;
    out.radius = quad.params.y;
    out.round = quad.round;
    return out;
}

// As in quad.slang and render/shape.rs.
float roundedRectDistance(float2 p, float2 halfSize, float r)
{
    r = max(min(r, min(halfSize.x, halfSize.y)), 0.0);
    float2 q = abs(p) - halfSize + r;
    return length(max(q, float2(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

[shader("fragment")]
float4 fragmentMain(Varyings v) : SV_Target
{
    int2 texel = int2(floor(v.position.xy - v.dest.xy));
    float a = v.opacity;
    if (v.radius > 0.0)
    {
        // SV_Position is the pixel centre, as in the software renderer.
        float2 halfSize = v.round.zw * 0.5;
        float2 p = v.position.xy - (v.round.xy + halfSize);
        a *= saturate(0.5 - roundedRectDistance(p, halfSize, v.radius));
    }
    return image.Load(int3(texel, 0)) * a;
}
