'use strict'

/* ============================================================
   Pisper 宣传页脚本
   1) GPU 粒子场:整页背景,随区块 morph 成形
   2) 页面交互:导航、标签页、复制、滚动显现
   两块逻辑解耦:粒子引擎不可用时页面完整可用
   ============================================================ */

const reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches
const finePointer = window.matchMedia('(pointer: fine)').matches

const clamp = (v, a, b) => (v < a ? a : v > b ? b : v)
const lerp = (a, b, t) => a + (b - a) * t
// 帧率无关平滑,高刷屏与低刷屏手感一致
const damp = (a, b, l, dt) => lerp(a, b, 1 - Math.exp(-l * dt))

/* ============================================================
   粒子场引擎(WebGL2 transform feedback)
   ============================================================ */

const FIELD_UPDATE_VS = `#version 300 es
precision highp float;

layout(location=0) in vec4 aPosVel; // xy=位置 zw=速度
layout(location=1) in vec2 aTarget; // 归巢目标
layout(location=2) in float aSeed;  // 个体差异

uniform float uDt;
uniform float uTime;
uniform float uSpringK;
uniform float uDamp;
uniform float uWobble;
uniform float uMouseForce;
uniform float uSwirl;
uniform float uScatter;
uniform vec2  uMouse;
uniform vec4  uBlast;   // xy=爆心 z=强度
uniform float uAspect;

out vec4 vPosVel;

// 错相正弦流场:廉价的「生命力」扰动
vec2 flow(vec2 p, float t){
  return vec2(
    sin(p.y * 3.1 + t * 0.9) + sin(p.y * 7.3 - t * 1.7) * 0.5,
    cos(p.x * 2.7 - t * 0.8) + cos(p.x * 6.1 + t * 1.3) * 0.5
  );
}

void main(){
  vec2 pos = aPosVel.xy;
  vec2 vel = aPosVel.zw;

  // 归巢弹簧:形状成形的核心力
  vec2 F = (aTarget - pos) * uSpringK * (0.6 + aSeed * 0.8);
  F += flow(pos * (1.0 + aSeed), uTime + aSeed * 17.0) * uWobble;

  // 换形/滚动冲击:沿种子方向的随机踢,制造「散开再聚拢」
  F += vec2(sin(aSeed * 127.1 + uTime), cos(aSeed * 311.7 - uTime)) * uScatter;

  // 鼠标力场:径向推/拉 + 切向漩涡
  vec2 md = pos - uMouse;
  float d = length(md);
  float R = 0.30;
  if (d < R && d > 1e-4){
    float f = 1.0 - d / R;
    vec2 dir = md / d;
    F += dir * f * f * uMouseForce;
    F += vec2(-dir.y, dir.x) * f * uSwirl;
  }

  // 定点爆破:高斯衰减的径向冲击波
  float bl = length(pos - uBlast.xy);
  if (uBlast.z > 0.001 && bl > 1e-4)
    F += (pos - uBlast.xy) / bl * exp(-bl * bl * 5.0) * uBlast.z;

  // 椭圆软边界:防止粒子永久飞出视野
  float far = length(vec2(pos.x / max(uAspect, 1e-3), pos.y));
  if (far > 1.5) F -= (pos / max(length(pos), 1e-3)) * (far - 1.5) * 26.0;

  vel = (vel + F * uDt) * clamp(1.0 - uDamp * uDt, 0.0, 1.0);
  pos += vel * uDt;
  vPosVel = vec4(pos, vel);
}`

// transform feedback 程序只需要顶点着色器,挂个空 FS 兼容保守驱动
const FIELD_UPDATE_FS = `#version 300 es
precision mediump float;
out vec4 o;
void main(){ o = vec4(0.0); }`

const FIELD_RENDER_VS = `#version 300 es
precision highp float;

layout(location=0) in vec4 aPosVel;
layout(location=2) in float aSeed;

uniform float uDpr;
uniform float uAspect;
uniform vec3 uColA; // 基色(慢速)
uniform vec3 uColB; // 热色(高速)
uniform vec3 uColC; // 点缀(种子)

out vec3 vColor;

void main(){
  vec2 pos = aPosVel.xy;
  float speed = length(aPosVel.zw);
  gl_Position = vec4(pos.x / uAspect, pos.y, 0.0, 1.0);
  float t = clamp(speed * 0.06, 0.0, 1.0);
  vec3 col = mix(uColA, uColB, t);
  vColor = mix(col, uColC, aSeed * aSeed * 0.55);
  gl_PointSize = (1.3 + aSeed * 2.4 + t * 2.2) * uDpr;
}`

const FIELD_RENDER_FS = `#version 300 es
precision mediump float;
in vec3 vColor;
out vec4 frag;
void main(){
  float d = length(gl_PointCoord - 0.5);
  float a = smoothstep(0.5, 0.04, d);
  frag = vec4(vColor * a * 0.8, 1.0); // 预乘亮度,配合 ONE,ONE 加法混合
}`

// 全屏三角形:gl_VertexID 免顶点缓冲
const FIELD_FADE_VS = `#version 300 es
void main(){
  vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
  gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}`

const FIELD_FADE_FS = `#version 300 es
precision mediump float;
uniform float uFade;
uniform vec3 uBg;
out vec4 frag;
void main(){ frag = vec4(uBg, uFade); }`

function createField(canvas) {
  const gl = canvas.getContext('webgl2', {
    alpha: false,
    antialias: false,
    depth: false,
    stencil: false,
    powerPreference: 'high-performance',
  })
  if (!gl) return null

  const compile = (type, src) => {
    const s = gl.createShader(type)
    gl.shaderSource(s, src)
    gl.compileShader(s)
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) {
      console.error(gl.getShaderInfoLog(s))
      return null
    }
    return s
  }
  const makeProgram = (vsSrc, fsSrc, tfOut) => {
    const p = gl.createProgram()
    gl.attachShader(p, compile(gl.VERTEX_SHADER, vsSrc))
    gl.attachShader(p, compile(gl.FRAGMENT_SHADER, fsSrc))
    if (tfOut) gl.transformFeedbackVaryings(p, tfOut, gl.INTERLEAVED_ATTRIBS)
    gl.linkProgram(p)
    if (!gl.getProgramParameter(p, gl.LINK_STATUS)) {
      console.error(gl.getProgramInfoLog(p))
      return null
    }
    return p
  }
  const U = (prog, names) => {
    const o = {}
    for (const n of names) o[n] = gl.getUniformLocation(prog, n)
    return o
  }

  const progUpdate = makeProgram(FIELD_UPDATE_VS, FIELD_UPDATE_FS, ['vPosVel'])
  const uUpd = U(progUpdate, [
    'uDt', 'uTime', 'uSpringK', 'uDamp', 'uWobble',
    'uMouseForce', 'uSwirl', 'uScatter', 'uMouse', 'uBlast', 'uAspect',
  ])
  const progRender = makeProgram(FIELD_RENDER_VS, FIELD_RENDER_FS)
  const uRen = U(progRender, ['uDpr', 'uAspect', 'uColA', 'uColB', 'uColC'])
  const progFade = makeProgram(FIELD_FADE_VS, FIELD_FADE_FS)
  const uFad = U(progFade, ['uFade', 'uBg'])
  const emptyVAO = gl.createVertexArray()

  // 品牌三色:慢速暗紫 → 高速柠檬绿,种子粒子点缀青
  const PAL = {
    bg: [0.043, 0.047, 0.047],
    a: [0.09, 0.08, 0.15],
    b: [0.875, 1.0, 0.38],
    c: [0.38, 0.82, 0.86],
  }

  let worldAspect = 1
  let reduced = reduceMotion
  let sys = null
  let src = 0
  let current = null // { word, origin } 供 resize 后重采样

  const makeBuf = (data, usage) => {
    const b = gl.createBuffer()
    gl.bindBuffer(gl.ARRAY_BUFFER, b)
    gl.bufferData(gl.ARRAY_BUFFER, data, usage)
    return b
  }

  function buildSystem(N) {
    const prevPts = sys ? sys.lastPts : null // 画质重建不能丢形状
    if (sys) {
      for (const b of sys.stateBufs) gl.deleteBuffer(b)
      gl.deleteBuffer(sys.targetBuf)
      gl.deleteBuffer(sys.seedBuf)
      for (const v of sys.vaos) gl.deleteVertexArray(v)
      gl.deleteTransformFeedback(sys.tf)
    }
    const seeds = new Float32Array(N)
    const state = new Float32Array(N * 4)
    for (let i = 0; i < N; i++) {
      seeds[i] = Math.random()
      // 初始撒在圆盘里,首次成形时从四周飞入,自带开场戏
      const a = Math.random() * 6.2832
      const r = Math.sqrt(Math.random()) * 1.3
      state[i * 4] = Math.cos(a) * r * worldAspect
      state[i * 4 + 1] = Math.sin(a) * r
    }
    sys = {
      N,
      stateBufs: [makeBuf(state, gl.DYNAMIC_COPY), makeBuf(state, gl.DYNAMIC_COPY)],
      targetBuf: makeBuf(new Float32Array(N * 2), gl.DYNAMIC_DRAW),
      seedBuf: makeBuf(seeds, gl.STATIC_DRAW),
      vaos: [],
      tf: gl.createTransformFeedback(),
      lastPts: null,
    }
    for (let i = 0; i < 2; i++) {
      const v = gl.createVertexArray()
      gl.bindVertexArray(v)
      gl.bindBuffer(gl.ARRAY_BUFFER, sys.stateBufs[i])
      gl.enableVertexAttribArray(0)
      gl.vertexAttribPointer(0, 4, gl.FLOAT, false, 0, 0)
      gl.bindBuffer(gl.ARRAY_BUFFER, sys.targetBuf)
      gl.enableVertexAttribArray(1)
      gl.vertexAttribPointer(1, 2, gl.FLOAT, false, 0, 0)
      gl.bindBuffer(gl.ARRAY_BUFFER, sys.seedBuf)
      gl.enableVertexAttribArray(2)
      gl.vertexAttribPointer(2, 1, gl.FLOAT, false, 0, 0)
      sys.vaos.push(v)
    }
    gl.bindVertexArray(null)
    if (prevPts) applyTargets(prevPts)
  }

  function applyTargets(pts) {
    sys.lastPts = pts
    const N = sys.N
    const n = pts.length / 2
    const arr = new Float32Array(N * 2)
    for (let i = 0; i < N; i++) {
      const j = (i % n) * 2
      arr[i * 2] = pts[j] + (Math.random() - 0.5) * 0.004
      arr[i * 2 + 1] = pts[j + 1] + (Math.random() - 0.5) * 0.004
    }
    gl.bindBuffer(gl.ARRAY_BUFFER, sys.targetBuf)
    gl.bufferData(gl.ARRAY_BUFFER, arr, gl.DYNAMIC_DRAW)
  }

  /* ---------- 文字采样:离屏 canvas 栅格化 ---------- */
  const textCanvas = document.createElement('canvas')
  textCanvas.width = 1200
  textCanvas.height = 600
  const textCtx = textCanvas.getContext('2d', { willReadFrequently: true })

  function rasterToPoints(W, H, originX) {
    const img = textCtx.getImageData(0, 0, W, H).data
    const raw = []
    const step = 3 // 采样步长:密度与点数平衡
    for (let y = 0; y < H; y += step)
      for (let x = 0; x < W; x += step)
        if (img[(y * W + x) * 4 + 3] > 128) raw.push(x, y)
    if (raw.length < 8) return null
    // Fisher-Yates 洗牌(按点对),避免取模复用时出现条带
    for (let i = raw.length - 2; i > 0; i -= 2) {
      const j = ((Math.random() * ((i >> 1) + 1)) | 0) << 1
      const tx = raw[i]
      raw[i] = raw[j]
      raw[j] = tx
      const ty = raw[i + 1]
      raw[i + 1] = raw[j + 1]
      raw[j + 1] = ty
    }
    let spanY = 1.42
    let spanX = spanY * (W / H)
    const maxHalfX = worldAspect * 0.94 - Math.abs(originX)
    if (spanX / 2 > maxHalfX && maxHalfX > 0.3) {
      const s = (maxHalfX * 2) / spanX
      spanX *= s
      spanY *= s
    }
    const out = new Float32Array(raw.length)
    for (let i = 0; i < raw.length; i += 2) {
      out[i] = (raw[i] / W - 0.5) * spanX + originX
      out[i + 1] = (0.5 - raw[i + 1] / H) * spanY
    }
    return out
  }

  function sampleText(str, originX) {
    const W = textCanvas.width
    const H = textCanvas.height
    const c = textCtx
    c.clearRect(0, 0, W, H)
    c.fillStyle = '#fff'
    c.textAlign = 'center'
    c.textBaseline = 'middle'
    let size = H * 0.58
    const setFont = (s) =>
      (c.font = `900 ${s}px "Avenir Next","Helvetica Neue","PingFang SC","Hiragino Sans GB","Microsoft YaHei",sans-serif`)
    setFont(size)
    const maxW = W * 0.9
    while (c.measureText(str).width > maxW && size > 24) {
      size *= 0.93
      setFont(size)
    }
    c.fillText(str, W / 2, H / 2 + size * 0.04)
    return rasterToPoints(W, H, originX)
  }

  /* ---------- 交互状态 ---------- */
  const mouse = { x: 999, y: 999 } // 初始丢到远处,避免开场无谓扰动
  let holding = false
  let scatter = 0
  const blast = { x: 0, y: 0, s: 0 }

  /* ---------- 尺寸与自适应画质 ---------- */
  const tiers = [100000, 70000, 45000, 28000, 16000]
  const startTier = finePointer ? 1 : 3
  let tierIdx = reduced ? tiers.length - 1 : startTier
  let lastQChange = 0
  let fps = 60

  function resize() {
    const dpr = Math.min(window.devicePixelRatio || 1, 1.5)
    canvas.width = Math.round(window.innerWidth * dpr)
    canvas.height = Math.round(window.innerHeight * dpr)
    worldAspect = canvas.width / canvas.height
    gl.viewport(0, 0, canvas.width, canvas.height)
    gl.clearColor(PAL.bg[0], PAL.bg[1], PAL.bg[2], 1)
    gl.clear(gl.COLOR_BUFFER_BIT) // 重置拖尾,防止 resize 后残影
    if (current) applyTargets(sampleText(current.word, current.origin) || sys.lastPts)
  }

  /* ---------- 主循环 ---------- */
  let last = performance.now()
  let simTime = 0
  let rafId = 0
  let running = false

  function frame(now) {
    rafId = requestAnimationFrame(frame)
    const dt = clamp((now - last) / 1000, 0.0001, 0.05)
    last = now

    simTime += dt * (reduced ? 0.15 : 1) // calm 模式慢速呼吸而非完全冻结
    scatter *= Math.exp(-7 * dt)
    blast.s *= Math.exp(-5 * dt)

    fps = damp(fps, 1 / dt, 1.2, dt)
    if (now - lastQChange > 3000) {
      if (fps < 40 && tierIdx < tiers.length - 1) {
        tierIdx++
        lastQChange = now
        buildSystem(tiers[tierIdx])
      } else if (fps > 58 && tierIdx > startTier && !reduced) {
        tierIdx--
        lastQChange = now
        buildSystem(tiers[tierIdx])
      }
    }

    const dpr = Math.min(window.devicePixelRatio || 1, 1.5)

    // —— 更新 pass:光栅化关闭,纯粹的状态递推 ——
    gl.useProgram(progUpdate)
    gl.uniform1f(uUpd.uDt, Math.min(dt, 0.033))
    gl.uniform1f(uUpd.uTime, simTime)
    gl.uniform1f(uUpd.uSpringK, reduced ? 10.0 : 7.0)
    gl.uniform1f(uUpd.uDamp, 3.4)
    gl.uniform1f(uUpd.uWobble, reduced ? 0.08 : 0.5)
    gl.uniform1f(uUpd.uMouseForce, holding ? -34.0 : 42.0)
    gl.uniform1f(uUpd.uSwirl, holding ? 30.0 : 0.0)
    gl.uniform1f(uUpd.uScatter, scatter)
    gl.uniform2f(uUpd.uMouse, mouse.x, mouse.y)
    gl.uniform4f(uUpd.uBlast, blast.x, blast.y, blast.s, 0)
    gl.uniform1f(uUpd.uAspect, worldAspect)

    const dst = 1 - src
    gl.bindVertexArray(sys.vaos[src])
    gl.bindTransformFeedback(gl.TRANSFORM_FEEDBACK, sys.tf)
    gl.bindBufferBase(gl.TRANSFORM_FEEDBACK_BUFFER, 0, sys.stateBufs[dst])
    gl.enable(gl.RASTERIZER_DISCARD)
    gl.beginTransformFeedback(gl.POINTS)
    gl.drawArrays(gl.POINTS, 0, sys.N)
    gl.endTransformFeedback()
    gl.disable(gl.RASTERIZER_DISCARD)
    gl.bindTransformFeedback(gl.TRANSFORM_FEEDBACK, null)
    gl.bindBufferBase(gl.TRANSFORM_FEEDBACK_BUFFER, 0, null)

    // —— 拖尾 pass:半透明底色盖住上一帧 ——
    gl.enable(gl.BLEND)
    gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA)
    gl.useProgram(progFade)
    gl.uniform1f(uFad.uFade, reduced ? 0.5 : 0.18)
    gl.uniform3f(uFad.uBg, PAL.bg[0], PAL.bg[1], PAL.bg[2])
    gl.bindVertexArray(emptyVAO)
    gl.drawArrays(gl.TRIANGLES, 0, 3)

    // —— 渲染 pass:加法混合的发光点精灵 ——
    gl.blendFunc(gl.ONE, gl.ONE)
    gl.useProgram(progRender)
    gl.uniform1f(uRen.uDpr, dpr)
    gl.uniform1f(uRen.uAspect, worldAspect)
    gl.uniform3f(uRen.uColA, PAL.a[0], PAL.a[1], PAL.a[2])
    gl.uniform3f(uRen.uColB, PAL.b[0], PAL.b[1], PAL.b[2])
    gl.uniform3f(uRen.uColC, PAL.c[0], PAL.c[1], PAL.c[2])
    gl.bindVertexArray(sys.vaos[dst])
    gl.drawArrays(gl.POINTS, 0, sys.N)
    gl.bindVertexArray(null)

    src = dst // ping-pong
  }

  function start() {
    if (running) return
    running = true
    last = performance.now()
    rafId = requestAnimationFrame(frame)
  }
  function stop() {
    running = false
    cancelAnimationFrame(rafId)
  }

  document.addEventListener('visibilitychange', () => {
    if (document.hidden) stop()
    else start()
  })

  let resizeTimer = 0
  window.addEventListener(
    'resize',
    () => {
      clearTimeout(resizeTimer)
      resizeTimer = setTimeout(resize, 220) // 防抖,避免拖窗口时反复重采样
    },
    { passive: true },
  )

  resize()
  buildSystem(tiers[tierIdx])
  start()

  return {
    // 成形一个词:origin 为世界坐标 x 偏移(hero 居右,区块居中)
    morph(word, origin = 0) {
      const pts = sampleText(word, origin)
      if (!pts) return
      current = { word, origin }
      applyTargets(pts)
      if (!reduced) {
        scatter = Math.max(scatter, 22) // 换形冲击:散开再聚拢
        blast.x = origin
        blast.y = 0
        blast.s = 9
      }
    },
    // 滚动搅动:轻微打散,随即自愈
    stir(v) {
      if (!reduced) scatter = Math.min(scatter + v, 15)
    },
    pointer(x, y) {
      mouse.x = x
      mouse.y = y
    },
    hold(on, x, y) {
      holding = on
      if (on && !reduced) {
        blast.x = x
        blast.y = y
        blast.s = 12 // 按下即小爆,即时反馈
      }
    },
    blastAt(x, y, s) {
      if (reduced) return
      blast.x = x
      blast.y = y
      blast.s = s
    },
    setReduced(v) {
      reduced = v
    },
  }
}

const field = createField(document.querySelector('#field'))
if (!field) document.body.classList.add('no-field')

/* ============================================================
   粒子编排:hero 词轮播 + 区块 morph + 指针与滚动
   ============================================================ */

if (field) {
  const heroWords = ['PISPER', '开分支', 'BRANCH', 'PARALLEL', '跨设备']
  let heroIdx = 0
  let heroActive = true

  // 宽屏时把粒子词完整移到右侧舞台，避免与首屏文案和下载按钮重叠；窄屏居中。
  const heroOrigin = () =>
    window.innerWidth >= 1024 && window.innerWidth / window.innerHeight > 1.2
      ? (window.innerWidth / window.innerHeight) * 0.47
      : 0

  const morphHero = () => field.morph(heroWords[heroIdx % heroWords.length], heroOrigin())

  // 首成形:粒子从圆盘飞入,开场即高潮
  setTimeout(morphHero, 380)

  // hero 词轮播:只在 hero 可见时进行,离开即冻结
  setInterval(() => {
    if (!heroActive || reduceMotion || document.hidden) return
    heroIdx++
    morphHero()
  }, 5200)

  // 区块进入视口 → 粒子 morph 成对应词
  const morphIO = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue
        heroActive = false
        field.morph(entry.target.dataset.morph, 0)
      }
    },
    { threshold: 0.32 },
  )
  for (const el of document.querySelectorAll('[data-morph]')) morphIO.observe(el)

  // hero 回到视口 → 恢复词轮播
  const heroEl = document.querySelector('#top')
  if (heroEl) {
    new IntersectionObserver(
      ([entry]) => {
        const was = heroActive
        heroActive = entry.isIntersecting
        if (heroActive && !was) morphHero()
      },
      { threshold: 0.4 },
    ).observe(heroEl)
  }

  // 指针:移动驱散,按住吸附成涡,双击引爆
  const toWorld = (cx, cy) => [
    ((cx / window.innerWidth) * 2 - 1) * (window.innerWidth / window.innerHeight),
    -((cy / window.innerHeight) * 2 - 1),
  ]
  window.addEventListener(
    'pointermove',
    (e) => {
      const [x, y] = toWorld(e.clientX, e.clientY)
      field.pointer(x, y)
    },
    { passive: true },
  )
  const interactive = (t) => t.closest?.('a, button, input, select, textarea, [role="tab"]')
  window.addEventListener(
    'pointerdown',
    (e) => {
      if (interactive(e.target)) return // 不抢按钮与链接的点击
      const [x, y] = toWorld(e.clientX, e.clientY)
      field.hold(true, x, y)
    },
    { passive: true },
  )
  window.addEventListener('pointerup', () => field.hold(false), { passive: true })
  window.addEventListener('pointercancel', () => field.hold(false), { passive: true })
  window.addEventListener('dblclick', (e) => {
    if (interactive(e.target)) return
    const [x, y] = toWorld(e.clientX, e.clientY)
    field.blastAt(x, y, 85)
  })
}

/* ============================================================
   页面交互(与粒子解耦,引擎缺席时全部照常工作)
   ============================================================ */

// 导航滚动态 + 顶部进度条 + 滚动搅动粒子:共用一个监听
const header = document.querySelector('[data-header]')
const progressBar = document.querySelector('.scroll-progress')
let lastScrollY = window.scrollY
let scrollFrame = 0
window.addEventListener(
  'scroll',
  () => {
    const y = window.scrollY
    header?.classList.toggle('is-scrolled', y > 12)
    if (progressBar) {
      const max = document.documentElement.scrollHeight - window.innerHeight
      progressBar.style.width = `${max > 0 ? (y / max) * 100 : 0}%`
    }
    if (field) field.stir(Math.min(Math.abs(y - lastScrollY) * 0.045, 5))
    lastScrollY = y
    // 视差计算合并到下一帧,避免滚动事件里反复读写布局
    if (!scrollFrame) {
      scrollFrame = requestAnimationFrame(() => {
        scrollFrame = 0
        updateParallax()
      })
    }
  },
  { passive: true },
)

// 界面标签页:预加载截图,切换时淡入
const tabs = [...document.querySelectorAll('.product-tabs [role="tab"]')]
const panel = document.querySelector('#product-panel')
const productShot = document.querySelector('[data-product-shot]')
const productTitle = document.querySelector('[data-product-title]')
const productCopy = document.querySelector('[data-product-copy]')
let imageRequest = 0

function selectTab(tab, moveFocus = false) {
  if (!tab || tab.getAttribute('aria-selected') === 'true') return
  for (const candidate of tabs) {
    candidate.setAttribute('aria-selected', String(candidate === tab))
    candidate.tabIndex = candidate === tab ? 0 : -1
  }
  if (moveFocus) tab.focus()
  panel?.setAttribute('aria-labelledby', tab.id)
  productTitle.textContent = tab.dataset.title || ''
  productCopy.textContent = tab.dataset.copy || ''

  const request = ++imageRequest
  const nextImage = new Image()
  nextImage.decoding = 'async'
  productShot.classList.add('is-changing')
  nextImage.addEventListener('load', () => {
    if (request !== imageRequest) return
    productShot.src = nextImage.src
    productShot.alt = tab.dataset.alt || ''
    productShot.classList.remove('is-changing')
  })
  nextImage.addEventListener('error', () => {
    if (request === imageRequest) productShot.classList.remove('is-changing')
  })
  nextImage.src = tab.dataset.shot || ''
}

for (const [index, tab] of tabs.entries()) {
  tab.addEventListener('click', () => selectTab(tab))
  tab.addEventListener('keydown', (e) => {
    if (e.key !== 'ArrowRight' && e.key !== 'ArrowLeft') return
    e.preventDefault()
    const next = tabs[(index + (e.key === 'ArrowRight' ? 1 : -1) + tabs.length) % tabs.length]
    selectTab(next, true)
  })
}

// 复制 npm 安装命令
const copyButton = document.querySelector('[data-copy-install]')
if (copyButton) {
  const label = copyButton.querySelector('[data-copy-label]')
  const status = document.querySelector('[data-copy-status]')
  const command = document.querySelector('[data-install-command]')?.textContent || 'npm i -g pisper'
  let resetTimer = 0

  const writeClipboard = async (text) => {
    if (navigator.clipboard?.writeText) return navigator.clipboard.writeText(text)
    // 兜底:老浏览器用隐藏 textarea + execCommand
    const helper = document.createElement('textarea')
    helper.value = text
    helper.style.position = 'fixed'
    helper.style.opacity = '0'
    document.body.append(helper)
    helper.select()
    document.execCommand('copy')
    helper.remove()
  }

  copyButton.addEventListener('click', async () => {
    try {
      await writeClipboard(command)
      copyButton.classList.add('is-copied')
      label.textContent = '已复制'
      if (status) status.textContent = '已复制安装命令'
      clearTimeout(resetTimer)
      resetTimer = setTimeout(() => {
        copyButton.classList.remove('is-copied')
        label.textContent = '复制'
        if (status) status.textContent = ''
      }, 1600)
    } catch {
      if (status) status.textContent = '复制失败,请手动复制'
    }
  })
}

// 滚动显现:进入视口即入场,只播一次
if ('IntersectionObserver' in window && !reduceMotion) {
  const revealIO = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue
        entry.target.classList.add('in-view')
        revealIO.unobserve(entry.target)
      }
    },
    { threshold: 0.12 },
  )
  for (const el of document.querySelectorAll('.reveal')) revealIO.observe(el)
} else {
  for (const el of document.querySelectorAll('.reveal')) el.classList.add('in-view')
}

/* 滚动视差:给标注 data-depth 的元素按离屏幕中心的距离施加纵向偏移,
   形成前后分层。只写 CSS 变量,与 .reveal 的入场 transform 合成,互不打断。 */
const parallaxLayers = reduceMotion ? [] : [...document.querySelectorAll('[data-depth]')]
let parallaxVisible = []

if (parallaxLayers.length && 'IntersectionObserver' in window) {
  // 只对视口内的层做计算,长页面滚动时避免每帧遍历全部元素
  const parallaxIO = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) entry.target.dataset.inview = entry.isIntersecting ? '1' : ''
      parallaxVisible = parallaxLayers.filter((el) => el.dataset.inview === '1')
      for (const el of parallaxLayers) {
        if (el.dataset.inview !== '1') {
          el.style.setProperty('--py', '0px')
          el.classList.remove('is-parallax')
        } else {
          el.classList.add('is-parallax')
        }
      }
      updateParallax()
    },
    { rootMargin: '15% 0px' },
  )
  for (const el of parallaxLayers) parallaxIO.observe(el)
}

function updateParallax() {
  const viewH = window.innerHeight || 1
  for (const el of parallaxVisible) {
    const rect = el.getBoundingClientRect()
    // -1(刚从下方进入) → 0(居中) → 1(从上方离开)
    const progress = (rect.top + rect.height / 2 - viewH / 2) / (viewH / 2 + rect.height / 2)
    const depth = Number(el.dataset.depth) || 0
    el.style.setProperty('--py', `${(progress * depth * -1).toFixed(2)}px`)
  }
}

// 磁吸按钮
if (!reduceMotion) {
  for (const el of document.querySelectorAll('.magnetic')) {
    el.addEventListener('pointermove', (e) => {
      const rect = el.getBoundingClientRect()
      const x = e.clientX - (rect.left + rect.width / 2)
      const y = e.clientY - (rect.top + rect.height / 2)
      el.style.transform = `translate(${x * 0.16}px, ${y * 0.16}px)`
    })
    el.addEventListener('pointerleave', () => {
      el.style.transform = ''
    })
  }
}

// 截图面板轻微 3D 倾斜
if (!reduceMotion) {
  for (const card of document.querySelectorAll('.tilt-card')) {
    card.addEventListener('pointermove', (e) => {
      const rect = card.getBoundingClientRect()
      const px = (e.clientX - rect.left) / rect.width - 0.5
      const py = (e.clientY - rect.top) / rect.height - 0.5
      card.style.setProperty('--tilt-x', `${(-py * 3).toFixed(2)}deg`)
      card.style.setProperty('--tilt-y', `${(px * 3).toFixed(2)}deg`)
    })
    card.addEventListener('pointerleave', () => {
      card.style.setProperty('--tilt-x', '0deg')
      card.style.setProperty('--tilt-y', '0deg')
    })
  }
}

// 从受信任的 Release 清单生成直链；清单不可用时保留 Releases 页面兜底。
function appReleaseAssetUrl(releaseUrl, assetName) {
  const fileName = String(assetName || '').trim()
  if (!fileName || fileName.includes('/') || fileName.includes('\\')) return releaseUrl
  const releaseBase = releaseUrl.replace(/\/+$/, '')
  const assetBase = releaseBase.replace('/releases/tag/', '/releases/download/')
  if (assetBase === releaseBase) return releaseUrl
  return `${assetBase}/${encodeURIComponent(fileName)}`
}

const DESKTOP_RELEASE_API =
  'https://api.github.com/repos/ling-kong-ran/pisper/releases/latest'
const DESKTOP_RELEASE_PAGE = 'https://github.com/ling-kong-ran/pisper/releases/latest'

function detectDownloadTarget() {
  const ua = navigator.userAgent.toLowerCase()
  const platform = String(navigator.platform || '').toLowerCase()
  const touchDevice = Number(navigator.maxTouchPoints || 0) > 1

  if (/android/.test(ua)) return { type: 'android', label: '下载 Android 版' }
  if (/iphone|ipad|ipod/.test(ua) || (platform === 'macintel' && touchDevice)) {
    return { type: 'ios', label: '下载 iOS 版' }
  }
  if (/windows/.test(ua) || /^win/.test(platform)) return { type: 'windows', label: '下载 Windows 版' }
  if (/mac os|macintosh/.test(ua) || /^mac/.test(platform)) {
    return { type: 'macos', label: '下载 macOS 版' }
  }
  if (/linux/.test(ua) || /^linux/.test(platform)) return { type: 'linux', label: '下载 Linux 版' }
  return { type: 'unknown', label: '' }
}

// Chromium 能提供架构高熵值；其他浏览器无法区分 Apple 芯片时选择可由 Rosetta 运行的 x86 包。
async function detectMacArchitecture() {
  const userAgentData = navigator.userAgentData
  if (typeof userAgentData?.getHighEntropyValues === 'function') {
    try {
      const values = await userAgentData.getHighEntropyValues(['architecture'])
      if (/arm/i.test(values.architecture || '')) return 'aarch64'
    } catch {
      // 浏览器拒绝高熵值时使用兼容性更好的 x86_64 安装包。
    }
  }
  return /arm64|aarch64/i.test(navigator.userAgent) ? 'aarch64' : 'x86_64'
}

async function readJson(url) {
  const response = await fetch(url, { cache: 'no-store', headers: { Accept: 'application/json' } })
  if (!response.ok) throw new Error(`Download manifest request failed: ${response.status}`)
  return response.json()
}

async function desktopReleaseAssetUrl(target) {
  const release = await readJson(DESKTOP_RELEASE_API)
  const version = String(release?.tag_name || '').replace(/^v/, '')
  if (!/^\d+\.\d+\.\d+$/.test(version)) return DESKTOP_RELEASE_PAGE

  const architecture = target === 'macos' ? await detectMacArchitecture() : 'x86_64'
  const suffixes = {
    macos: `darwin_${architecture}.dmg`,
    windows: 'windows_x86_64-setup.exe',
    linux: 'linux_x86_64.AppImage',
  }
  const expectedName = `Pisper_${version}_${suffixes[target] || ''}`
  const asset = release.assets?.find((candidate) => candidate.name === expectedName)
  return asset?.browser_download_url || DESKTOP_RELEASE_PAGE
}

const downloadTarget = detectDownloadTarget()
const appManifestPromise = readJson('latest-app.json').catch(() => null)
const desktopAssetPromise =
  downloadTarget.type === 'windows' || downloadTarget.type === 'macos' || downloadTarget.type === 'linux'
    ? desktopReleaseAssetUrl(downloadTarget.type).catch(() => DESKTOP_RELEASE_PAGE)
    : Promise.resolve(DESKTOP_RELEASE_PAGE)

Promise.all([appManifestPromise, desktopAssetPromise]).then(([appManifest, desktopUrl]) => {
  const appReleaseUrl = String(appManifest?.url || '').trim()
  const appAssets = { android: appManifest?.apk, ios: appManifest?.ipa }
  const appUrl = appReleaseUrl
    ? appReleaseAssetUrl(appReleaseUrl, appAssets[downloadTarget.type])
    : DESKTOP_RELEASE_PAGE
  const targetUrl =
    downloadTarget.type === 'android' || downloadTarget.type === 'ios' ? appUrl : desktopUrl

  for (const link of document.querySelectorAll('[data-device-download]')) {
    link.href = targetUrl
    if (downloadTarget.label) {
      const label = link.querySelector('[data-download-label]')
      if (label) label.textContent = downloadTarget.label
    }
  }

  for (const link of document.querySelectorAll('[data-app-download]')) {
    const platform = link.dataset.appDownload
    const assetName = appAssets[platform] || link.dataset.appAsset
    if (appReleaseUrl) link.href = appReleaseAssetUrl(appReleaseUrl, assetName)
  }
  if (appReleaseUrl) {
    for (const link of document.querySelectorAll('[data-app-release]')) link.href = appReleaseUrl
  }
})

/* ============================================================
   滚动分页:轻拨一下滚轮 → 平滑跳到下一个停靠点
   ============================================================ */

/* 比视口高的区块(product / safety / mobile 都超过一屏)不能整块跳过,
   否则中间内容会被吞掉。这里按需要把长区块切成多个停靠点。 */
function computeSnapStops() {
  const headerH = document.querySelector('[data-header]')?.offsetHeight || 0
  const viewH = window.innerHeight
  const maxScroll = document.documentElement.scrollHeight - viewH
  const usable = viewH - headerH
  const stops = []

  for (const section of document.querySelectorAll('main > section')) {
    const top = section.getBoundingClientRect().top + window.scrollY
    const height = section.getBoundingClientRect().height
    // 顶部对齐到 header 下沿
    const start = Math.max(0, Math.round(top - headerH))
    // 短装饰带（如产品形态一行）不单独停靠，否则一次滚动只挑一小段，
    // 感觉不像“翻到下一页”；它会随相邻区块一起入镜。
    if (height >= usable * 0.4) stops.push(start)
    // 超过一屏的区块:补足中间停靠点,保证每屏内容都会被看到
    if (height > usable) {
      const parts = Math.ceil(height / usable)
      for (let i = 1; i < parts; i++) {
        stops.push(
          Math.round(Math.min(start + (height - usable) * (i / (parts - 1 || 1)), maxScroll)),
        )
      }
    }
  }

  stops.push(maxScroll)
  // 去重并丢掉过近的停靠点，避免一次滚动只挪一小段（阈值取半屏）
  const minGap = Math.max(160, usable * 0.5)
  return [...new Set(stops.filter((v) => v >= 0 && v <= maxScroll).sort((a, b) => a - b))].filter(
    (v, i, arr) => i === 0 || v === maxScroll || v - arr[i - 1] > minGap,
  )
}

let snapStops = computeSnapStops()
let snapTarget = null
let snapUntil = 0

function nearestStopIndex(y) {
  let best = 0
  let bestGap = Infinity
  for (let i = 0; i < snapStops.length; i++) {
    const gap = Math.abs(snapStops[i] - y)
    if (gap < bestGap) {
      bestGap = gap
      best = i
    }
  }
  return best
}

function snapTo(index) {
  const target = snapStops[Math.max(0, Math.min(snapStops.length - 1, index))]
  if (target == null) return
  snapTarget = target
  // 动画期间忽略后续滚轮,避免连跳;按距离给出上限,长距离不至于卡太久
  snapUntil = performance.now() + 900
  window.scrollTo({ top: target, behavior: 'smooth' })
}

// 仅接管"轻拨一下"的鼠标滚轮;触控板惯性、缩放与横向滚动一律放行
if (!reduceMotion && window.matchMedia('(min-width: 961px)').matches) {
  window.addEventListener('resize', () => {
    snapStops = computeSnapStops()
  })
  // 图片/字体加载完可能改变布局，停靠点需要重算，否则会按旧坐标跳错位置
  window.addEventListener('load', () => {
    snapStops = computeSnapStops()
  })

  window.addEventListener(
    'wheel',
    (event) => {
      if (event.ctrlKey || event.defaultPrevented) return
      if (Math.abs(event.deltaX) > Math.abs(event.deltaY)) return
      // deltaMode 0 且步长很小的连续事件多来自触控板/平滑滚轮,保留原生手感
      if (event.deltaMode === 0 && Math.abs(event.deltaY) < 16) return
      // 可滚动的内层容器(如代码块)优先处理自己的滚动
      for (let node = event.target; node && node !== document.body; node = node.parentElement) {
        if (!(node instanceof HTMLElement)) break
        const style = getComputedStyle(node)
        if (!/(auto|scroll)/.test(style.overflowY)) continue
        if (node.scrollHeight <= node.clientHeight + 1) continue
        const atTop = node.scrollTop <= 0
        const atBottom = node.scrollTop + node.clientHeight >= node.scrollHeight - 1
        if (!(event.deltaY < 0 ? atTop : atBottom)) return
      }

      const now = performance.now()
      if (now < snapUntil) {
        event.preventDefault()
        return
      }

      const dir = event.deltaY > 0 ? 1 : -1
      const y = window.scrollY
      const maxScroll = document.documentElement.scrollHeight - window.innerHeight
      // 已在两端时放行,便于露出页脚或回弹
      if ((dir > 0 && y >= maxScroll - 2) || (dir < 0 && y <= 2)) return

      const base = snapTarget != null && Math.abs(snapTarget - y) < 40 ? snapTarget : y
      let index = nearestStopIndex(base)
      // 距离当前停靠点较远时,先归位到本屏,再继续翻页
      if (Math.abs(snapStops[index] - base) > 24) {
        index = dir > 0 ? index + (snapStops[index] > base ? 0 : 1) : index - (snapStops[index] < base ? 0 : 1)
      } else {
        index += dir
      }

      event.preventDefault()
      snapTo(index)
    },
    { passive: false },
  )

  // 键盘翻页沿用同一套停靠点
  window.addEventListener('keydown', (event) => {
    if (event.defaultPrevented || event.ctrlKey || event.metaKey || event.altKey) return
    const tag = document.activeElement?.tagName
    if (tag === 'INPUT' || tag === 'TEXTAREA' || document.activeElement?.isContentEditable) return
    if (event.key !== 'PageDown' && event.key !== 'PageUp') return
    event.preventDefault()
    const index = nearestStopIndex(snapTarget != null ? snapTarget : window.scrollY)
    snapTo(index + (event.key === 'PageDown' ? 1 : -1))
  })
}
