/**
 * The call's glow on the composer's edge: a thin line of light with a short
 * halo, drawn by one fragment shader on a canvas that lies over the
 * composer's dock and takes no pointer. The halo fades out before the dock's
 * own edge, because a pane cuts off whatever is drawn past it and a glow cut
 * off is a hard line. One light circles the edge while
 * the call connects; the rim flashes once as it does, then follows whoever
 * is talking — sky blue for the person, periwinkle for the voice — and a
 * slow light circles again while the agent works. What each phase looks
 * like is `model/rim.ts`'s; this file only draws it.
 *
 * The canvas is laid over its dock (`.workspace-dock`) and measures the
 * composer's card inside it whenever either changes size, so the rim stays
 * on the card as the line above it grows or the pane is resized. With less motion nothing
 * circles and loudness is not followed (`stillRim`). Where WebGL is not to
 * be had the dock says so (`data-rim="plain"`) and the stylesheet draws a
 * still ring on the composer instead.
 */
import { useEffect, useRef, useState } from "react"
import { reducedMotion } from "../../adapters/motion-preference"
import type { VoicePhase } from "../model/phase"
import { easeRim, rimLook, stillRim, type RimLook } from "../model/rim"
import "./voice.css"

/** How near the canvas's edge the halo has faded to nothing, in CSS pixels. */
const EDGE_FADE = 10

const VERTEX = "attribute vec2 p;void main(){gl_Position=vec4(p,0.,1.);}"

/*
 * `d` is the distance to the composer's rounded edge, in CSS pixels. Outside
 * it the halo falls off over `spread`; on it a 1.5px rim is drawn; inside it
 * nothing is, so the composer's glass is never tinted. Noise is read around
 * the edge by angle, as a point on a circle, so it has no seam.
 */
const FRAGMENT = `precision mediump float;
uniform vec2 uRes;uniform vec4 uRect;
uniform float uRad,uTime,uLevel,uGlow,uSweep,uHue,uGrey,uBloom,uPhase,uDpr,uFade;
float sdRB(vec2 p,vec2 b,float r){vec2 q=abs(p)-b+r;return length(max(q,0.))+min(max(q.x,q.y),0.)-r;}
float hash(vec2 p){return fract(sin(dot(p,vec2(127.1,311.7)))*43758.5453);}
float noise(vec2 p){vec2 i=floor(p),f=fract(p);f=f*f*(3.-2.*f);
return mix(mix(hash(i),hash(i+vec2(1.,0.)),f.x),mix(hash(i+vec2(0.,1.)),hash(i+vec2(1.,1.)),f.x),f.y);}
float fbm(vec2 p){float v=0.,a=.5;for(int i=0;i<4;i++){v+=a*noise(p);p*=2.02;a*=.5;}return v;}
void main(){
vec2 p=gl_FragCoord.xy;p.y=uRes.y-p.y;vec2 q=p-uRect.xy;
float d=sdRB(q,uRect.zw,uRad)/uDpr;
float ang=atan(q.y/uRect.w,q.x/uRect.z);vec2 c=vec2(cos(ang),sin(ang));
float n=fbm(c*1.9+vec2(uTime*.1,-uTime*.08));
float n2=fbm(c*2.6+vec2(-uTime*.15,4.+uTime*.12));
float spread=(5.+16.*uLevel+12.*uBloom)*(.5+n);
float outside=smoothstep(-1.5,0.,d);
float glow=exp(-max(d,0.)/spread)*outside;
float rim=exp(-abs(d)/1.5);
float arc=pow(max(dot(c,vec2(cos(uPhase),sin(uPhase))),0.),7.);
float a=glow*uGlow*(.2+.8*n)*(.3+.5*uLevel)+arc*uSweep*(glow*.35+rim*.6)+rim*(uGlow*.5+uBloom*.5)+glow*uBloom*.2;
vec3 col=mix(vec3(.525,.72,1.),vec3(.70,.66,1.),clamp(uHue*.8+n2*.5-.15,0.,1.));
col=mix(col,vec3(1.),clamp(rim*.7+n2*.2,0.,1.));
col=mix(col,vec3(.62),uGrey);
vec2 e=min(p,uRes-p)/uDpr;a*=smoothstep(0.,uFade,min(e.x,e.y));
a=clamp(a,0.,1.);gl_FragColor=vec4(col*a,a);}`

const UNIFORMS = [
  "uRes",
  "uRect",
  "uRad",
  "uTime",
  "uLevel",
  "uGlow",
  "uSweep",
  "uHue",
  "uGrey",
  "uBloom",
  "uPhase",
  "uDpr",
  "uFade",
] as const

type Uniforms = Record<(typeof UNIFORMS)[number], WebGLUniformLocation | null>

/** The shader, compiled and ready to draw a full-canvas quad; undefined where WebGL is not to be had. */
function prepare(
  canvas: HTMLCanvasElement,
): { gl: WebGLRenderingContext; uniforms: Uniforms } | undefined {
  const gl = canvas.getContext("webgl", { premultipliedAlpha: true, antialias: false })
  if (!gl) return undefined
  const program = gl.createProgram()
  for (const [type, source] of [
    [gl.VERTEX_SHADER, VERTEX],
    [gl.FRAGMENT_SHADER, FRAGMENT],
  ] as const) {
    const shader = gl.createShader(type)
    if (!shader) return undefined
    gl.shaderSource(shader, source)
    gl.compileShader(shader)
    if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) return undefined
    gl.attachShader(program, shader)
  }
  gl.linkProgram(program)
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) return undefined
  gl.useProgram(program)
  gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer())
  gl.bufferData(
    gl.ARRAY_BUFFER,
    new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]),
    gl.STATIC_DRAW,
  )
  const at = gl.getAttribLocation(program, "p")
  gl.enableVertexAttribArray(at)
  gl.vertexAttribPointer(at, 2, gl.FLOAT, false, 0, 0)
  const uniforms = Object.fromEntries(
    UNIFORMS.map((name) => [name, gl.getUniformLocation(program, name)]),
  ) as Uniforms
  return { gl, uniforms }
}

const dark: RimLook = rimLook("idle", { input: 0, output: 0 }, true)

export function VoiceRim({
  phase,
  microphoneOn,
  levels,
}: {
  phase: VoicePhase
  microphoneOn: boolean
  /** How loud the person and the voice are now, 0–1. */
  levels: () => { readonly input: number; readonly output: number }
}) {
  const ref = useRef<HTMLCanvasElement>(null)
  // What the frame loop reads: the newest props, without restarting the loop.
  const now = useRef({ phase, microphoneOn, levels })
  now.current = { phase, microphoneOn, levels }
  const [plain, setPlain] = useState(false)

  useEffect(() => {
    const canvas = ref.current
    const dock = canvas?.closest<HTMLElement>(".workspace-dock")
    if (!canvas || !dock) return
    const drawing = prepare(canvas)
    if (!drawing) {
      setPlain(true)
      return
    }
    const { gl, uniforms } = drawing
    let frame = 0
    const lost = () => {
      cancelAnimationFrame(frame)
      setPlain(true)
    }
    canvas.addEventListener("webglcontextlost", lost)
    // Where the card is on the canvas, in CSS pixels; measured when the dock
    // or the card changes size, not every frame.
    let card:
      | { x: number; y: number; halfWidth: number; halfHeight: number; radius: number }
      | undefined
    let watched: HTMLElement | null = null
    const sizes = new ResizeObserver(() => measure())
    const measure = () => {
      const form = dock.querySelector<HTMLElement>("form.desktop-composer")
      if (form !== watched) {
        if (watched) sizes.unobserve(watched)
        if (form) sizes.observe(form)
        watched = form
      }
      if (!form) {
        card = undefined
        return
      }
      const over = canvas.getBoundingClientRect()
      const box = form.getBoundingClientRect()
      card = {
        x: box.left - over.left + box.width / 2,
        y: box.top - over.top + box.height / 2,
        halfWidth: box.width / 2,
        halfHeight: box.height / 2,
        radius: Number.parseFloat(getComputedStyle(form).borderTopLeftRadius) || 0,
      }
    }
    sizes.observe(dock)
    measure()
    const began = performance.now()
    let shown = dark
    let was: VoicePhase = "idle"
    let bloom = 0
    let turn = 0
    let last = performance.now()
    const tick = (time: number) => {
      frame = requestAnimationFrame(tick)
      const seconds = Math.min(0.05, (time - last) / 1000)
      last = time
      const { phase, microphoneOn, levels } = now.current
      const still = reducedMotion()
      // Connected: one flash as the circling light lets go.
      if (was === "connecting" && phase !== "connecting" && !still) bloom = 1
      was = phase
      bloom = Math.max(0, bloom - seconds * 1.4)
      shown = still
        ? stillRim(phase, microphoneOn)
        : easeRim(shown, rimLook(phase, levels(), microphoneOn), seconds)
      turn += seconds * shown.spin

      if (!card) return
      const scale = Math.min(window.devicePixelRatio || 1, 2)
      const width = Math.round(canvas.clientWidth * scale)
      const height = Math.round(canvas.clientHeight * scale)
      if (canvas.width !== width || canvas.height !== height) {
        canvas.width = width
        canvas.height = height
      }
      gl.viewport(0, 0, width, height)
      gl.clearColor(0, 0, 0, 0)
      gl.clear(gl.COLOR_BUFFER_BIT)
      gl.uniform2f(uniforms.uRes, width, height)
      gl.uniform4f(
        uniforms.uRect,
        card.x * scale,
        card.y * scale,
        card.halfWidth * scale,
        card.halfHeight * scale,
      )
      gl.uniform1f(uniforms.uRad, card.radius * scale)
      gl.uniform1f(uniforms.uDpr, scale)
      gl.uniform1f(uniforms.uFade, EDGE_FADE)
      // Counted from when the rim appeared: a float this small keeps its detail.
      gl.uniform1f(uniforms.uTime, still ? 0 : (time - began) / 1000)
      gl.uniform1f(uniforms.uLevel, shown.level)
      gl.uniform1f(uniforms.uGlow, shown.glow)
      gl.uniform1f(uniforms.uSweep, shown.sweep)
      gl.uniform1f(uniforms.uHue, shown.hue)
      gl.uniform1f(uniforms.uGrey, shown.grey)
      gl.uniform1f(uniforms.uBloom, bloom * bloom)
      gl.uniform1f(uniforms.uPhase, turn)
      gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4)
    }
    frame = requestAnimationFrame(tick)
    return () => {
      cancelAnimationFrame(frame)
      sizes.disconnect()
      canvas.removeEventListener("webglcontextlost", lost)
      // The context is left to go with its canvas: an effect run again on
      // the same canvas (React's strict mode) is handed the same context,
      // and one thrown away here could draw nothing then.
    }
  }, [])

  return (
    <canvas
      ref={ref}
      className="voice-rim"
      data-phase={phase}
      data-rim={plain ? "plain" : "drawn"}
      aria-hidden
    />
  )
}
