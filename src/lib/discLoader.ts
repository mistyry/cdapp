// 启动加载动画：按 logo 光盘建模渲染。
// 盘面为同心刻槽，镜面项用 Kajiya-Kay 各向异性高光，切线取刻槽方向。两盏固定方向光，正交相机。
// 光源与相机不动，画面变化只来自盘面位姿：斜向正弦漂移，以及随之起伏的俯仰、偏转与横滚。
// 静止位姿对应 logo 的 rotate(-16) scale(1 .84)，此时画面与 icon.svg 的盘面一致。

const D2R = Math.PI / 180;
const REST = { roll: -16, pitch: Math.acos(0.84) / D2R, yaw: 0 };

type M3 = number[][];

function poseMatrix(roll: number, pitch: number, yaw: number): M3 {
  const r = roll * D2R;
  const p = pitch * D2R;
  const y = yaw * D2R;
  const rz = [[Math.cos(r), -Math.sin(r), 0], [Math.sin(r), Math.cos(r), 0], [0, 0, 1]];
  const ry = [[Math.cos(y), 0, Math.sin(y)], [0, 1, 0], [-Math.sin(y), 0, Math.cos(y)]];
  const rx = [[1, 0, 0], [0, Math.cos(p), -Math.sin(p)], [0, Math.sin(p), Math.cos(p)]];
  const mul = (a: M3, b: M3) => a.map((row) => b[0].map((_, j) => row.reduce((s, v, k) => s + v * b[k][j], 0)));
  return mul(mul(rz, ry), rx);
}

// 列主序，供 GLSL mat3 使用。
const cols = (m: M3) => [m[0][0], m[1][0], m[2][0], m[0][1], m[1][1], m[2][1], m[0][2], m[1][2], m[2][2]];
const apply = (m: M3, v: number[]) => m.map((row) => row[0] * v[0] + row[1] * v[1] + row[2] * v[2]);

// 由 logo 静止画面反推的光照：静止盘面局部坐标中，两条亮带轴向分别为 -103° 与 -138°。
const M0 = poseMatrix(REST.roll, REST.pitch, REST.yaw);
const hLocal = (deg: number) => [Math.cos(deg * D2R) * 0.6, Math.sin(deg * D2R) * 0.6, 0.8];
const HA = apply(M0, hLocal(-103));
const HB = apply(M0, hLocal(-138));
// 光强衰减方向，固定在世界空间。
const DIR_A = [Math.cos(-103 * D2R), Math.sin(-103 * D2R)];
const DIR_B = [Math.cos(42 * D2R), Math.sin(42 * D2R)];

const VS = `#version 300 es
in vec2 p; void main() { gl_Position = vec4(p, 0, 1); }`;

const FS = `#version 300 es
precision highp float;
uniform vec2 uRes; uniform vec2 uDrift;
uniform mat3 uM; uniform mat3 uRest;
uniform vec3 uHA, uHB; uniform vec2 uDirA, uDirB;
uniform sampler2D uMask;
out vec4 o;
const float R = 174.0, THICK = 13.9, EXP = 44.0, TH = 0.5;
vec2 toLocal(vec2 rel, vec3 n, vec3 u, vec3 v, out vec3 P) {
  float z = -(n.x * rel.x + n.y * rel.y) / n.z;
  P = vec3(rel, z);
  return vec2(dot(P, u), dot(P, v));
}
float mask(vec2 l) { return texture(uMask, (l + 256.0 - 82.0) / 348.0).r; }
float kk(vec3 T, vec3 H) { float d = dot(T, H); return pow(max(0.0, 1.0 - d * d), EXP * 0.5); }
void main() {
  vec2 g = vec2(60.0 + gl_FragCoord.x / uRes.x * 392.0, 60.0 + (1.0 - gl_FragCoord.y / uRes.y) * 392.0);
  vec2 c = vec2(256.0) + uDrift;
  vec3 u = uM[0], v = uM[1], n = uM[2];
  vec3 P, Pb;
  vec2 lt = toLocal(g - c, n, u, v, P);
  vec2 lb = toLocal(g - c + THICK * n.xy, n, u, v, Pb);
  float r = length(lt), rb = length(lb);
  float aa = fwidth(r);
  float aT = 1.0 - smoothstep(R - aa, R + aa, r);
  float aB = 1.0 - smoothstep(R - aa, R + aa, rb);
  // 表情印在反光面上，只按顶面坐标镂空
  float face = mix(1.0, mask(lt), aT);

  // 世界空间中的固定照明场，以静止盘面坐标表示
  vec3 W = vec3(g - 256.0, P.z);
  vec2 gr = vec2(dot(W, uRest[0]), dot(W, uRest[1]));
  float d = length(gr + 256.0 - vec2(179.4, 144.6)) / 348.0;
  vec3 base = d < 0.63 ? mix(vec3(1.0), vec3(0.976, 0.988, 1.0), d / 0.63)
                       : mix(vec3(0.976, 0.988, 1.0), vec3(0.867, 0.933, 1.0), (d - 0.63) / 0.37);

  vec2 tl = vec2(-lt.y, lt.x) / max(r, 1e-3);
  vec3 T = u * tl.x + v * tl.y;
  float iA = kk(T, uHA), iB = kk(T, uHB);
  float hub = smoothstep(34.0, 36.0, r);
  float e = max(fwidth(iA) + fwidth(iB), 1e-3);
  float sA = smoothstep(-e * 400.0, e * 400.0, dot(gr, uDirA));
  float sB = smoothstep(-e * 400.0, e * 400.0, dot(gr, uDirB));
  float bA = smoothstep(TH - e, TH + e, iA) * hub;
  float bB = smoothstep(TH - e, TH + e, iB) * hub;
  float white = max(bA * sA, bB * sB);
  float pale = max(bA * (1.0 - sA), bB * (1.0 - sB));

  vec3 col = base;
  col = mix(col, vec3(0.796, 0.902, 0.988), 0.43 * pale);
  col = mix(col, vec3(1.0), 0.92 * white);
  float ring = 1.0 - smoothstep(1.5 - aa, 1.5 + aa, abs(r - 161.5));
  col = mix(col, vec3(1.0), 0.8 * ring);

  vec3 edge = vec3(0.749, 0.875, 0.976);
  float a = aT + aB * (1.0 - aT);
  vec3 rgb = col * aT + edge * aB * (1.0 - aT);
  float aS = (1.0 - smoothstep(R - aa, R + aa, length(lt - vec2(0.0, 10.0)))) * 0.13;
  vec4 disc = vec4(rgb * face, a * face);
  vec4 sh = vec4(vec3(0.133, 0.514, 0.816) * aS, aS);
  o = disc + sh * (1.0 - disc.a);
}`;

const FACE = `<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="82 82 348 348">
<rect width="512" height="512" fill="white"/>
<g fill="black">
<path d="M160 216H214.5C228 216 238.5 226.5 238.5 239.5S228 263 214.5 263H160A8.5 8.5 0 0 1 160 246H192.5V233H160A8.5 8.5 0 0 1 160 216Z"/>
<path d="M160 216H214.5C228 216 238.5 226.5 238.5 239.5S228 263 214.5 263H160A8.5 8.5 0 0 1 160 246H192.5V233H160A8.5 8.5 0 0 1 160 216Z" transform="translate(123.2 0)"/>
</g>
<path d="M226 280.5C227 288.2 232 292.6 239.7 292.6C247.4 292.6 252.7 287.2 254 280.5C255.3 287.2 260.6 292.6 268.3 292.6C276 292.6 281 288.2 282 280.5" fill="none" stroke="black" stroke-width="14.7" stroke-linecap="round" stroke-linejoin="round"/>
</svg>`;

export interface DiscFrame {
  dx: number;
  dy: number;
}

export const AMP = 1.5;

// 位姿随时间变化，t 为秒，amp 为姿态摆幅倍数。
export function pose(t: number, amp = AMP) {
  const tau = Math.PI * 2;
  const ph1 = (t * tau) / 3.2;
  const ph2 = (t * tau) / 2.3 + 0.8;
  return {
    dx: 7 * Math.sin(ph1),
    dy: 9 * Math.sin(ph2),
    pitch: REST.pitch + amp * 6 * Math.cos(ph2),
    yaw: amp * 8 * Math.cos(ph1),
    roll: REST.roll + amp * 3 * Math.sin(ph1 + 0.6),
  };
}

export const restPose = () => ({ dx: 0, dy: 0, ...REST });

function loadFace(): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = reject;
    img.src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(FACE)}`;
  });
}

// 在 canvas 上绘制光盘。onFrame 回传每帧漂移量，用于同步阴影。
// 返回停止函数；WebGL2 不可用或初始化失败时返回 null，由调用方显示静态图标。
export async function startDisc(
  canvas: HTMLCanvasElement,
  opts: { still: boolean; onFrame?: (f: DiscFrame) => void },
): Promise<(() => void) | null> {
  const gl = canvas.getContext("webgl2", { premultipliedAlpha: true, antialias: true });
  if (!gl) return null;
  const compile = (type: number, src: string) => {
    const s = gl.createShader(type);
    if (!s) return null;
    gl.shaderSource(s, src);
    gl.compileShader(s);
    return gl.getShaderParameter(s, gl.COMPILE_STATUS) ? s : null;
  };
  const vs = compile(gl.VERTEX_SHADER, VS);
  const fs = compile(gl.FRAGMENT_SHADER, FS);
  const prog = gl.createProgram();
  if (!vs || !fs || !prog) return null;
  gl.attachShader(prog, vs);
  gl.attachShader(prog, fs);
  gl.linkProgram(prog);
  if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) return null;
  gl.useProgram(prog);

  let img: HTMLImageElement;
  try {
    img = await loadFace();
  } catch {
    return null;
  }

  gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
  const loc = gl.getAttribLocation(prog, "p");
  gl.enableVertexAttribArray(loc);
  gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
  gl.bindTexture(gl.TEXTURE_2D, gl.createTexture());
  gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, img);
  gl.generateMipmap(gl.TEXTURE_2D);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);

  const u = (n: string) => gl.getUniformLocation(prog, n);
  gl.uniform2f(u("uRes"), canvas.width, canvas.height);
  gl.uniformMatrix3fv(u("uRest"), false, cols(M0));
  gl.uniform3fv(u("uHA"), HA);
  gl.uniform3fv(u("uHB"), HB);
  gl.uniform2fv(u("uDirA"), DIR_A);
  gl.uniform2fv(u("uDirB"), DIR_B);
  gl.viewport(0, 0, canvas.width, canvas.height);
  const uDrift = u("uDrift");
  const uM = u("uM");

  const draw = (p: ReturnType<typeof restPose>) => {
    gl.uniform2f(uDrift, p.dx, p.dy);
    gl.uniformMatrix3fv(uM, false, cols(poseMatrix(p.roll, p.pitch, p.yaw)));
    gl.clearColor(0, 0, 0, 0);
    gl.clear(gl.COLOR_BUFFER_BIT);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
    opts.onFrame?.({ dx: p.dx, dy: p.dy });
  };

  let raf = 0;
  if (opts.still) {
    draw(restPose());
  } else {
    const t0 = performance.now();
    const frame = (now: number) => {
      draw(pose((now - t0) / 1000));
      raf = requestAnimationFrame(frame);
    };
    raf = requestAnimationFrame(frame);
  }
  return () => {
    cancelAnimationFrame(raf);
    gl.getExtension("WEBGL_lose_context")?.loseContext();
  };
}
