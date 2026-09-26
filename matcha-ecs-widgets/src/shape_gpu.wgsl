struct Params { size: vec4<f32>, radius: vec4<f32>, border: vec4<f32>, direction: vec4<i32> };
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var input_image: texture_2d<f32>;
@vertex fn vertex(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32> {
    let v=array<vec2<f32>,3>(vec2<f32>(-1.,-1.),vec2<f32>(3.,-1.),vec2<f32>(-1.,3.));return vec4<f32>(v[i],0.,1.);
}
fn sdf(point:vec2<f32>,half_size:vec2<f32>,corners:vec4<f32>)->f32 {
    var r=corners.x;
    if point.x>=0. {r=select(corners.y,corners.z,point.y>=0.);} else if point.y>=0. {r=corners.w;}
    r=clamp(r,0.,min(half_size.x,half_size.y));
    let q=abs(point)-(half_size-vec2<f32>(r));return length(max(q,vec2<f32>(0.)))+min(max(q.x,q.y),0.)-r;
}
@fragment fn shape(@builtin(position) pixel:vec4<f32>)->@location(0) vec4<f32> {
    let point=pixel.xy-p.size.xy*0.5;
    let half_size=max(p.size.xy*0.5-vec2<f32>(p.size.z),vec2<f32>(0.));
    var c=clamp(0.5-sdf(point,half_size,p.radius),0.,1.);
    if any(p.border>vec4<f32>(0.)) {
        let t=p.border.x;let r=p.border.y;let b=p.border.z;let l=p.border.w;
        let inner_half=max(half_size-vec2<f32>(l+r,t+b)*0.5,vec2<f32>(0.));
        let inner_center=vec2<f32>(l-r,t-b)*0.5;
        let inner_radius=max(p.radius-vec4<f32>(max(l,t),max(r,t),max(r,b),max(l,b)),vec4<f32>(0.));
        c=clamp(c-clamp(0.5-sdf(point-inner_center,inner_half,inner_radius),0.,1.),0.,1.);
    }
    return vec4<f32>(round(c*255.)/255.,0.,0.,1.);
}
@fragment fn blur(@builtin(position) pixel:vec4<f32>)->@location(0) vec4<f32> {
    let xy=vec2<i32>(pixel.xy);let maximum=vec2<i32>(textureDimensions(input_image))-vec2<i32>(1);
    let radius=i32(p.size.w);var sum=0u;
    for(var i=-radius;i<=radius;i++) {
        let at=clamp(xy+p.direction.xy*i,vec2<i32>(0),maximum);
        sum+=u32(round(textureLoad(input_image,at,0).r*255.));
    }
    return vec4<f32>(f32(sum/u32(2*radius+1))/255.,0.,0.,1.);
}
