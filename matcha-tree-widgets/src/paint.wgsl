@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
struct Out { @builtin(position) p:vec4<f32>, @location(0) color:vec4<f32>, @location(1) uv:vec2<f32> }
@vertex fn vertex(@location(0) p:vec4<f32>,@location(1) color:vec4<f32>,@location(2) uv:vec2<f32>)->Out { return Out(p,color,uv); }
@fragment fn color(input:Out)->@location(0) vec4<f32> { return vec4<f32>(input.color.rgb*input.color.a,input.color.a); }
@fragment fn image(input:Out)->@location(0) vec4<f32> { return textureSample(source,image_sampler,input.uv); }
