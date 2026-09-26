struct Params { transform:mat4x4<f32>, viewport_size:vec4<f32>, mode:vec4<u32> };
@group(0) @binding(0) var background:texture_2d<f32>;
@group(0) @binding(1) var output:texture_storage_2d<rgba8unorm,write>;
@group(0) @binding(2) var<uniform> p:Params;
fn read_at(point:vec2<i32>)->vec4<f32> {return textureLoad(background,clamp(point,vec2<i32>(0),vec2<i32>(textureDimensions(background))-vec2<i32>(1)),0);}
@compute @workgroup_size(8,8) fn main(@builtin(global_invocation_id) id:vec3<u32>) {
    let size=textureDimensions(output);if any(id.xy>=size) {return;}
    let uv=(vec2<f32>(id.xy)+0.5)/vec2<f32>(size);var color=vec4<f32>(0.);
    if p.mode.x==0u {
        color=select(vec4<f32>(0.02,0.12,0.24,1.),vec4<f32>(0.9,0.45,0.06,1.),(id.x/8u+id.y/8u)%2u==0u);
    } else {
        let world=p.transform*vec4<f32>(uv*p.viewport_size.zw,0.,1.);
        let point=vec2<i32>(world.xy/world.w/p.viewport_size.xy*vec2<f32>(textureDimensions(background)));
        if p.mode.x==1u {for(var y=-3;y<=3;y++) {for(var x=-3;x<=3;x++) {color+=read_at(point+vec2<i32>(x,y))/49.;}}}
        else if p.mode.x==2u {color=read_at(point+vec2<i32>(i32(5.*sin(uv.y*40.)),0));}
        else {let base=read_at(point);color=vec4<f32>(vec3<f32>(base.a)-base.rgb,base.a);}
    }
    textureStore(output,vec2<i32>(id.xy),color);
}
