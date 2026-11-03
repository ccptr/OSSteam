// SPDX-License-Identifier: GPL-2.0-or-later
// Rounded rectangles with borders, and shadows (scene::Quad). One instance
// per quad, drawn as a four-vertex triangle strip shrunk to its clip. The
// coverage is the same as src/render/shape.rs; keep the two in step.

struct Push
{
    float2 viewport;
};

[[vk::push_constant]]
ConstantBuffer<Push> push;

struct Instance
{
    [[vk::location(0)]] float4 bounds;      // x, y, width, height
    [[vk::location(1)]] float4 clip;        // x0, y0, x1, y1
    [[vk::location(2)]] float4 background;  // straight alpha
    [[vk::location(3)]] float4 borderColor; // straight alpha
    [[vk::location(4)]] float4 params;      // corner radius, border width, blur
};

struct Varyings
{
    float4 position : SV_Position;
    nointerpolation float4 bounds : BOUNDS;
    nointerpolation float4 background : BACKGROUND;
    nointerpolation float4 borderColor : BORDER_COLOR;
    nointerpolation float4 params : PARAMS;
};

[shader("vertex")]
Varyings vertexMain(Instance quad, uint vertex : SV_VulkanVertexID)
{
    float2 corner = float2(float(vertex & 1), float(vertex >> 1));
    float grow = quad.params.z > 0.0 ? 3.0 * quad.params.z : 0.0;
    float2 p0 = max(quad.bounds.xy - grow, floor(quad.clip.xy));
    float2 p1 = min(quad.bounds.xy + quad.bounds.zw + grow, ceil(quad.clip.zw));
    p1 = max(p1, p0);
    float2 pos = lerp(p0, p1, corner);

    Varyings out;
    out.position = float4(pos / push.viewport * 2.0 - 1.0, 0.0, 1.0);
    out.bounds = quad.bounds;
    out.background = quad.background;
    out.borderColor = quad.borderColor;
    out.params = quad.params;
    return out;
}

float roundedRectDistance(float2 p, float2 halfSize, float r)
{
    r = max(min(r, min(halfSize.x, halfSize.y)), 0.0);
    float2 q = abs(p) - halfSize + r;
    return length(max(q, float2(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

// Abramowitz and Stegun 7.1.26, as shape::erf.
float erfApprox(float x)
{
    float a = abs(x);
    float t = 1.0 / (1.0 + 0.3275911 * a);
    float y = 1.0 - (((((1.0614054 * t - 1.453152) * t) + 1.4214137) * t - 0.28449674) * t + 0.2548296) * t * exp(-a * a);
    return sign(x) * y;
}

[shader("fragment")]
float4 fragmentMain(Varyings v) : SV_Target
{
    float2 halfSize = v.bounds.zw * 0.5;
    // SV_Position is the pixel centre, as in the software renderer.
    float2 p = v.position.xy - (v.bounds.xy + halfSize);
    float radius = v.params.x;
    float borderWidth = v.params.y;
    float blur = v.params.z;
    float d = roundedRectDistance(p, halfSize, radius);
    if (blur > 0.0)
    {
        float a = v.background.a * 0.5 * (1.0 - erfApprox(d / (1.41421356 * blur)));
        return float4(v.background.rgb * a, a);
    }
    float outer = saturate(0.5 - d);
    float inner = 1.0;
    if (borderWidth > 0.0)
    {
        float di = roundedRectDistance(p, halfSize - borderWidth, radius - borderWidth);
        inner = saturate(0.5 - di);
    }
    float3 rgb = (v.background.rgb * v.background.a * inner + v.borderColor.rgb * v.borderColor.a * (1.0 - inner)) * outer;
    float a = (v.background.a * inner + v.borderColor.a * (1.0 - inner)) * outer;
    return float4(rgb, a);
}
