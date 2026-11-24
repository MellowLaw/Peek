export type CameraEnhanceMode = "off" | "natural" | "studio";

export const ENHANCE_MODES: CameraEnhanceMode[] = ["off", "natural", "studio"];

export function nextEnhanceMode(cur: CameraEnhanceMode): CameraEnhanceMode {
  const idx = ENHANCE_MODES.indexOf(cur);
  return ENHANCE_MODES[(idx + 1) % ENHANCE_MODES.length];
}

export function normalizeEnhanceMode(raw: string | null | undefined): CameraEnhanceMode {
  if (raw === "natural" || raw === "studio") return raw;
  return "off";
}

export function fallbackCssFilter(mode: CameraEnhanceMode): string {
  if (mode === "natural") {
    return "brightness(1.1) contrast(1.07) saturate(1.1)";
  }
  if (mode === "studio") {
    return "brightness(1.18) contrast(1.12) saturate(1.16)";
  }
  return "none";
}

export async function openBestCameraStream(deviceId?: string): Promise<MediaStream> {
  if (!navigator.mediaDevices?.getUserMedia) {
    throw new Error("Camera API unavailable in WebView");
  }
  const baseConstraints: MediaTrackConstraints = {
    width: { ideal: 1920 },
    height: { ideal: 1080 },
    frameRate: { ideal: 30 },
  };
  if (deviceId) {
    try {
      return await navigator.mediaDevices.getUserMedia({
        video: { ...baseConstraints, deviceId: { exact: deviceId } },
        audio: false,
      });
    } catch {
      // Fall back to default device constraints if the saved deviceId is unavailable
    }
  }
  return await navigator.mediaDevices.getUserMedia({
    video: baseConstraints,
    audio: false,
  });
}

type EnhancePresetUniforms = {
  denoiseRadius: number;
  rangeSharpness: number;
  sharpen: number;
  exposure: number;
  shadowLift: number;
  contrast: number;
  vibrance: number;
  wb: [number, number, number];
  vignette: number;
};

const PRESET_UNIFORMS: Record<Exclude<CameraEnhanceMode, "off">, EnhancePresetUniforms> = {
  natural: {
    denoiseRadius: 1.0,
    rangeSharpness: 38.0,
    sharpen: 0.24,
    exposure: 1.08,
    shadowLift: 0.2,
    contrast: 0.14,
    vibrance: 0.14,
    wb: [1.03, 0.995, 0.975],
    vignette: 0.0,
  },
  studio: {
    denoiseRadius: 1.25,
    rangeSharpness: 24.0,
    sharpen: 0.36,
    exposure: 1.15,
    shadowLift: 0.3,
    contrast: 0.22,
    vibrance: 0.2,
    wb: [1.045, 0.992, 0.96],
    vignette: 0.22,
  },
};

const VERT_SHADER = `
attribute vec2 a_pos;
varying vec2 v_uv;
void main() {
  v_uv = a_pos * 0.5 + 0.5;
  gl_Position = vec4(a_pos, 0.0, 1.0);
}
`;

const FRAG_SHADER = `
precision mediump float;
varying vec2 v_uv;
uniform sampler2D u_tex;
uniform vec2 u_texel;
uniform vec2 u_uvScale;
uniform float u_mirror;
uniform float u_denoiseRadius;
uniform float u_rangeSharpness;
uniform float u_sharpen;
uniform float u_exposure;
uniform float u_shadowLift;
uniform float u_contrast;
uniform float u_vibrance;
uniform vec3 u_wb;
uniform float u_vignette;

void main() {
  vec2 baseUv = vec2(u_mirror > 0.5 ? 1.0 - v_uv.x : v_uv.x, v_uv.y);
  vec2 uv = clamp((baseUv - 0.5) * u_uvScale + 0.5, 0.001, 0.999);

  vec3 center = texture2D(u_tex, uv).rgb;
  vec3 acc = vec3(0.0);
  float wSum = 0.0;

  for (int dy = -1; dy <= 1; dy++) {
    for (int dx = -1; dx <= 1; dx++) {
      vec2 offset = vec2(float(dx), float(dy)) * u_texel * u_denoiseRadius;
      vec3 s = texture2D(u_tex, clamp(uv + offset, 0.001, 0.999)).rgb;
      vec3 diff = s - center;
      float spatial = (dx == 0 && dy == 0) ? 1.0 : ((abs(dx) + abs(dy)) == 1 ? 0.72 : 0.48);
      float range = exp(-dot(diff, diff) * u_rangeSharpness);
      float w = spatial * range;
      acc += s * w;
      wSum += w;
    }
  }
  vec3 denoised = acc / max(wSum, 0.001);

  vec2 blurStep = u_texel * 1.5;
  vec3 crossAvg = (
    texture2D(u_tex, clamp(uv + vec2(blurStep.x, 0.0), 0.001, 0.999)).rgb +
    texture2D(u_tex, clamp(uv - vec2(blurStep.x, 0.0), 0.001, 0.999)).rgb +
    texture2D(u_tex, clamp(uv + vec2(0.0, blurStep.y), 0.001, 0.999)).rgb +
    texture2D(u_tex, clamp(uv - vec2(0.0, blurStep.y), 0.001, 0.999)).rgb
  ) * 0.25;
  vec3 color = clamp(denoised + clamp(denoised - crossAvg, -0.16, 0.16) * u_sharpen, 0.0, 1.0);

  color *= u_wb * u_exposure;
  float luma = dot(color, vec3(0.2126, 0.7152, 0.0722));
  color += u_shadowLift * pow(1.0 - clamp(luma, 0.0, 1.0), 2.2) * (color + 0.06);
  color = min(vec3(1.0), color / (vec3(1.0) + max(color - 0.78, 0.0) * 0.6));

  color = mix(color, color * color * (3.0 - 2.0 * color), u_contrast);
  float maxC = max(color.r, max(color.g, color.b));
  float minC = min(color.r, min(color.g, color.b));
  float sat = maxC - minC;
  float luma2 = dot(color, vec3(0.2126, 0.7152, 0.0722));
  color = mix(vec3(luma2), color, 1.0 + u_vibrance * (1.0 - sat));

  if (u_vignette > 0.0) {
    vec2 dist = v_uv - 0.5;
    color *= 1.0 - dot(dist, dist) * u_vignette;
  }

  gl_FragColor = vec4(clamp(color, 0.0, 1.0), 1.0);
}
`;

function compileShader(gl: WebGLRenderingContext, type: number, src: string): WebGLShader {
  const shader = gl.createShader(type);
  if (!shader) throw new Error("Failed to create WebGL shader");
  gl.shaderSource(shader, src);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    const info = gl.getShaderInfoLog(shader) || "Shader compile error";
    gl.deleteShader(shader);
    throw new Error(info);
  }
  return shader;
}

export class CameraGlRenderer {
  private gl: WebGLRenderingContext;
  private program: WebGLProgram;
  private buffer: WebGLBuffer;
  private texture: WebGLTexture;
  private uniforms: Record<string, WebGLUniformLocation | null>;
  private disposed = false;

  constructor(canvas: HTMLCanvasElement, preserveDrawingBuffer = false) {
    const opts: WebGLContextAttributes = {
      alpha: false,
      antialias: false,
      depth: false,
      stencil: false,
      preserveDrawingBuffer,
      powerPreference: "low-power",
    };
    const gl =
      (canvas.getContext("webgl2", opts) as WebGLRenderingContext | null) ||
      (canvas.getContext("webgl", opts) as WebGLRenderingContext | null);
    if (!gl) {
      throw new Error("WebGL unavailable");
    }
    this.gl = gl;

    const vs = compileShader(gl, gl.VERTEX_SHADER, VERT_SHADER);
    const fs = compileShader(gl, gl.FRAGMENT_SHADER, FRAG_SHADER);
    const program = gl.createProgram();
    if (!program) {
      gl.deleteShader(vs);
      gl.deleteShader(fs);
      throw new Error("Failed to create WebGL program");
    }
    gl.attachShader(program, vs);
    gl.attachShader(program, fs);
    gl.linkProgram(program);
    gl.deleteShader(vs);
    gl.deleteShader(fs);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
      const info = gl.getProgramInfoLog(program) || "Program link error";
      gl.deleteProgram(program);
      throw new Error(info);
    }
    this.program = program;

    const buffer = gl.createBuffer();
    if (!buffer) {
      gl.deleteProgram(program);
      throw new Error("Failed to create vertex buffer");
    }
    this.buffer = buffer;
    gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);

    const posLoc = gl.getAttribLocation(program, "a_pos");
    gl.enableVertexAttribArray(posLoc);
    gl.vertexAttribPointer(posLoc, 2, gl.FLOAT, false, 0, 0);

    const texture = gl.createTexture();
    if (!texture) {
      gl.deleteBuffer(buffer);
      gl.deleteProgram(program);
      throw new Error("Failed to create video texture");
    }
    this.texture = texture;
    gl.bindTexture(gl.TEXTURE_2D, texture);
    gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, 1);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);

    this.uniforms = {
      u_tex: gl.getUniformLocation(program, "u_tex"),
      u_texel: gl.getUniformLocation(program, "u_texel"),
      u_uvScale: gl.getUniformLocation(program, "u_uvScale"),
      u_mirror: gl.getUniformLocation(program, "u_mirror"),
      u_denoiseRadius: gl.getUniformLocation(program, "u_denoiseRadius"),
      u_rangeSharpness: gl.getUniformLocation(program, "u_rangeSharpness"),
      u_sharpen: gl.getUniformLocation(program, "u_sharpen"),
      u_exposure: gl.getUniformLocation(program, "u_exposure"),
      u_shadowLift: gl.getUniformLocation(program, "u_shadowLift"),
      u_contrast: gl.getUniformLocation(program, "u_contrast"),
      u_vibrance: gl.getUniformLocation(program, "u_vibrance"),
      u_wb: gl.getUniformLocation(program, "u_wb"),
      u_vignette: gl.getUniformLocation(program, "u_vignette"),
    };
  }

  render(
    video: HTMLVideoElement,
    mode: Exclude<CameraEnhanceMode, "off">,
    mirrored: boolean,
    cover = true,
  ): boolean {
    if (this.disposed || video.readyState < 2 || video.videoWidth <= 0 || video.videoHeight <= 0) {
      return false;
    }
    const { gl } = this;
    const canvas = gl.canvas as HTMLCanvasElement;
    const vw = video.videoWidth;
    const vh = video.videoHeight;

    let targetW = vw;
    let targetH = vh;
    let uvScaleX = 1.0;
    let uvScaleY = 1.0;

    if (cover) {
      const rect = canvas.getBoundingClientRect();
      const dpr = Math.min(window.devicePixelRatio || 1, 2);
      const cssW = Math.max(1, Math.round(rect.width));
      const cssH = Math.max(1, Math.round(rect.height));
      targetW = Math.max(1, Math.min(vw, Math.round(cssW * dpr)));
      targetH = Math.max(1, Math.min(vh, Math.round(cssH * dpr)));
      const videoAspect = vw / vh;
      const canvasAspect = cssW / cssH;
      if (videoAspect > canvasAspect) {
        uvScaleX = canvasAspect / videoAspect;
      } else {
        uvScaleY = videoAspect / canvasAspect;
      }
    }

    if (canvas.width !== targetW || canvas.height !== targetH) {
      canvas.width = targetW;
      canvas.height = targetH;
    }

    gl.viewport(0, 0, canvas.width, canvas.height);
    gl.useProgram(this.program);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.texture);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, video);

    const preset = PRESET_UNIFORMS[mode];
    gl.uniform1i(this.uniforms.u_tex, 0);
    gl.uniform2f(this.uniforms.u_texel, 1 / vw, 1 / vh);
    gl.uniform2f(this.uniforms.u_uvScale, uvScaleX, uvScaleY);
    gl.uniform1f(this.uniforms.u_mirror, mirrored ? 1.0 : 0.0);
    gl.uniform1f(this.uniforms.u_denoiseRadius, preset.denoiseRadius);
    gl.uniform1f(this.uniforms.u_rangeSharpness, preset.rangeSharpness);
    gl.uniform1f(this.uniforms.u_sharpen, preset.sharpen);
    gl.uniform1f(this.uniforms.u_exposure, preset.exposure);
    gl.uniform1f(this.uniforms.u_shadowLift, preset.shadowLift);
    gl.uniform1f(this.uniforms.u_contrast, preset.contrast);
    gl.uniform1f(this.uniforms.u_vibrance, preset.vibrance);
    gl.uniform3f(this.uniforms.u_wb, preset.wb[0], preset.wb[1], preset.wb[2]);
    gl.uniform1f(this.uniforms.u_vignette, preset.vignette);

    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
    return true;
  }

  dispose() {
    if (this.disposed) return;
    this.disposed = true;
    const { gl } = this;
    gl.deleteTexture(this.texture);
    gl.deleteBuffer(this.buffer);
    gl.deleteProgram(this.program);
    const lose = gl.getExtension("WEBGL_lose_context");
    lose?.loseContext();
  }
}

export function captureEnhancedSnapshot(
  video: HTMLVideoElement,
  mirrored: boolean,
  mode: CameraEnhanceMode,
): string | null {
  if (video.videoWidth === 0 || video.videoHeight === 0) return null;

  if (mode !== "off") {
    let glRenderer: CameraGlRenderer | null = null;
    try {
      const offscreen = document.createElement("canvas");
      offscreen.width = video.videoWidth;
      offscreen.height = video.videoHeight;
      glRenderer = new CameraGlRenderer(offscreen, true);
      if (glRenderer.render(video, mode, mirrored, false)) {
        return offscreen.toDataURL("image/png");
      }
    } catch {
      // Fall through to 2D canvas filter capture
    } finally {
      glRenderer?.dispose();
    }
  }

  const canvas = document.createElement("canvas");
  canvas.width = video.videoWidth;
  canvas.height = video.videoHeight;
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  if (mode !== "off") {
    ctx.filter = fallbackCssFilter(mode);
  }
  if (mirrored) {
    ctx.translate(canvas.width, 0);
    ctx.scale(-1, 1);
  }
  ctx.drawImage(video, 0, 0, canvas.width, canvas.height);
  return canvas.toDataURL("image/png");
}
