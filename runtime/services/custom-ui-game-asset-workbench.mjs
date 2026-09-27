// 独立项目由素材领域管理；组件仅通过受限桥调用共享处理能力。
import { GAME_ASSET_FRAME_EDITOR_SCRIPT } from './custom-ui-game-asset-frame-editor.mjs'
/** @type {import('./custom-ui-builtins.mjs').BuiltinCustomUiComponent} */
export const GAME_ASSET_WORKBENCH_COMPONENT = Object.freeze({
  id: 'pisper-game-asset-workbench',
  manifest: Object.freeze({
    name: 'Game Asset Workbench',
    version: '1.0.0',
    description:
      '游戏素材工作台：生成动作、逐帧编辑并导出图集 / Independent game asset projects and frame editing',
    entry: 'index.html',
    permissions: Object.freeze(['game-assets.read', 'game-assets.write', 'game-assets.run']),
  }),
  assets: Object.freeze({
    'frame-editor.js': GAME_ASSET_FRAME_EDITOR_SCRIPT,
    'index.html': String.raw`<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>Game Asset Workbench</title>
<style>
  :root { font: 14px/1.5 -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif; color-scheme: light dark; }
  * { box-sizing: border-box; }
  [hidden] { display: none !important; }
  body { margin: 0; min-width: 0; color: var(--text,#202124); background: var(--bg,#fafafa); }
  main { max-width: 960px; margin: auto; padding: clamp(16px,2.6vw,32px); padding-bottom: max(40px,env(safe-area-inset-bottom)); }
  .sr-only { position: absolute; width: 1px; height: 1px; padding: 0; margin: -1px; overflow: hidden; clip: rect(0,0,0,0); white-space: nowrap; border: 0; }
  h1,h2,p { margin: 0; }
  h2,legend { font-size: 15px; font-weight: 600; letter-spacing: -.01em; }
  button,input,select,textarea { font: inherit; color: inherit; }
  button,input:not([type=file]),select,textarea { border: 1px solid var(--stroke,#ddd); border-radius: 10px; background: var(--solid,#fff); }
  button { min-height: 40px; padding: 8px 13px; cursor: pointer; font-weight: 500; white-space: nowrap; transition: background .16s,border-color .16s,transform .16s; }
  button:hover:not(:disabled) { border-color: var(--stroke-hover,#bbb); background: var(--surface-hover,#f0f0f0); }
  button:active:not(:disabled) { transform: translateY(1px); }
  button:disabled { opacity: .48; cursor: default; }
  button.primary { background: var(--accent-strong,#222); color: var(--on-accent,#fff); border-color: transparent; }
  button.primary:hover:not(:disabled) { background: var(--accent-strong,#222); filter: brightness(1.12); }
  button:focus-visible,input:focus-visible,select:focus-visible,textarea:focus-visible,summary:focus-visible,.upload-zone:focus-visible { outline: 2px solid var(--focus,#777); outline-offset: 3px; }
  input:not([type=checkbox]):not([type=file]),select,textarea { width: 100%; min-width: 0; padding: 9px 11px; }
  input:not([type=checkbox]):not([type=file]):hover,select:hover,textarea:hover { border-color: var(--stroke-hover,#bbb); }
  textarea { resize: vertical; min-height: 92px; line-height: 1.55; }
  input[type=checkbox] { accent-color: var(--accent-strong,#222); width: 16px; height: 16px; flex-shrink: 0; }
  label { display: grid; gap: 7px; min-width: 0; font-weight: 500; }
  .bar,.row { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; }
  .muted { color: var(--text-secondary,#666); font-size: 12.5px; font-weight: 400; line-height: 1.5; }
  .bar { margin-bottom: 18px; padding: 10px; border: 1px solid var(--stroke-soft,var(--stroke,#ddd)); border-radius: 14px; background: var(--panel,#fff); }
  .bar select { width: auto; flex: 1 1 210px; min-width: 0; font-weight: 500; }
  .columns { display: grid; grid-template-columns: minmax(0,1fr); align-items: start; gap: clamp(14px,1.6vw,22px); }
  .stack { display: grid; gap: clamp(14px,1.6vw,22px); min-width: 0; }
  .card { min-width: 0; padding: clamp(16px,1.65vw,22px); border: 1px solid var(--stroke-soft,var(--stroke,#ddd)); border-radius: 16px; background: var(--panel,#fff); box-shadow: 0 8px 28px -28px #0008; }
  .card > h2 { display: flex; align-items: center; gap: 10px; padding-bottom: 14px; border-bottom: 1px solid var(--stroke-soft,var(--stroke,#ddd)); }
  .card > h2::before { content: attr(data-step); display: inline-grid; place-items: center; width: 27px; height: 27px; flex: none; border-radius: 8px; color: var(--accent-strong,#333); background: var(--accent-soft,var(--surface-muted,#eee)); font-size: 11px; letter-spacing: 0; }
  .fields { display: grid; gap: 15px; }
  .pair { display: grid; grid-template-columns: repeat(2,minmax(0,1fr)); gap: 12px; }
  .upload-zone { display: grid; gap: 0; position: relative; cursor: pointer; border: 1px dashed var(--stroke-hover,var(--stroke,#bbb)); border-radius: 14px; overflow: hidden; background: var(--surface-subtle,var(--solid,#fff)); transition: border-color .16s,background .16s,box-shadow .16s; }
  .upload-zone:hover:not([aria-disabled=true]),.upload-zone:focus-visible { border-color: var(--accent-strong,#555); background: var(--accent-soft,var(--surface-subtle,#f7f7f7)); }
  .upload-zone[aria-disabled=true] { cursor: default; opacity: .72; }
  #upload { position: absolute; width: 1px; height: 1px; padding: 0; margin: -1px; border: 0; opacity: 0; overflow: hidden; clip-path: inset(50%); pointer-events: none; }
  .source { display: grid; place-items: center; width: 100%; min-height: 220px; max-height: 330px; aspect-ratio: 4/3; overflow: hidden; background-color: var(--solid,#fff); background-image: repeating-conic-gradient(#8882 0% 25%,transparent 0% 50%); background-size: 20px 20px; }
  .source img { display: block; width: 100%; height: 100%; padding: 10px; object-fit: contain; }
  .source-empty { display: grid; justify-items: center; gap: 9px; width: min(82%,290px); padding: 22px; border: 1px solid var(--stroke-soft,var(--stroke,#ddd)); border-radius: 14px; background: var(--panel,#fff); text-align: center; box-shadow: 0 12px 28px -24px #0009; }
  .source-empty svg { width: 25px; height: 25px; color: var(--accent-strong,#333); }
  .source-empty strong { font-size: 14px; font-weight: 600; }
  .source-empty .muted { overflow-wrap: anywhere; }
  .upload-footer { display: flex; justify-content: space-between; align-items: center; gap: 12px; min-width: 0; padding: 11px 13px; border-top: 1px solid var(--stroke-soft,var(--stroke,#ddd)); background: var(--panel,#fff); }
  .upload-file { display: grid; gap: 1px; min-width: 0; font-weight: 400; }
  .upload-file strong { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13px; font-weight: 600; }
  .upload-button { display: inline-flex; align-items: center; justify-content: center; min-height: 35px; padding: 7px 12px; flex: none; border: 1px solid var(--stroke,#ddd); border-radius: 9px; background: var(--solid,#fff); font-weight: 600; }
  .upload-zone:hover:not([aria-disabled=true]) .upload-button { border-color: var(--accent-border,var(--stroke-hover,#bbb)); }
  .directions { display: flex; flex-wrap: wrap; gap: 8px; }
  .directions label { display: flex; align-items: center; gap: 7px; padding: 7px 10px; border: 1px solid var(--stroke-soft,var(--stroke,#ddd)); border-radius: 9px; background: var(--surface-subtle,var(--solid,#fff)); font-weight: 500; cursor: pointer; }
  .directions label:has(input:checked) { border-color: var(--accent-border,var(--stroke,#ddd)); background: var(--accent-soft,var(--surface-hover,#f0f0f0)); }
  fieldset { min-width: 0; margin: 0; padding: 0; border: 0; }
  legend { margin-bottom: 10px; }
  .action { display: grid; gap: 8px; border: 1px solid var(--stroke-soft,var(--stroke,#ddd)); border-radius: 11px; padding: 11px; background: var(--surface-subtle,var(--solid,#fff)); }
  .action .row { flex-wrap: nowrap; }
  .action .row input[type=text] { flex: 1; }
  .action textarea { min-height: 66px; background: var(--panel,#fff); }
  .icon { min-width: 38px; padding: 7px; }
  .status { padding: 11px 13px; border: 1px solid var(--stroke-soft,var(--stroke,#ddd)); border-radius: 10px; background: var(--surface-subtle,var(--surface-hover,#f0f0f0)); font-size: 12.5px; overflow-wrap: anywhere; }
  .status:empty { display: none; }
  #message { margin-bottom: 15px; }
  #background-engine { padding: 12px; border: 1px solid var(--accent-border,var(--stroke,#ddd)); border-radius: 11px; background: var(--accent-soft,var(--surface-subtle,#f7f7f7)); }
  #background-engine button { justify-self: start; }
  details { border-top: 1px solid var(--stroke-soft,var(--stroke,#ddd)); padding-top: 12px; }
  summary { cursor: pointer; font-size: 13px; font-weight: 600; }
  details > .fields { margin-top: 13px; }
  .stage { width: min(100%,560px); min-height: 300px; aspect-ratio: 1; display: grid; place-items: center; overflow: hidden; margin-inline: auto; border: 1px solid var(--stroke-soft,var(--stroke,#ddd)); border-radius: 13px; background-color: var(--solid,#fff); background-image: linear-gradient(45deg,#8881 25%,transparent 25%),linear-gradient(-45deg,#8881 25%,transparent 25%),linear-gradient(45deg,transparent 75%,#8881 75%),linear-gradient(-45deg,transparent 75%,#8881 75%); background-size: 20px 20px; background-position: 0 0,0 10px,10px -10px,-10px 0; }
  .stage canvas { max-width: 100%; max-height: 100%; width: 100%; height: 100%; object-fit: contain; }
  .preview-empty { display: grid; justify-items: center; gap: 13px; width: min(80%,340px); padding: 30px 24px; border: 1px solid var(--stroke-soft,var(--stroke,#ddd)); border-radius: 16px; background: var(--panel,#fff); text-align: center; box-shadow: 0 18px 35px -32px #0009; color: var(--text-secondary,#666); }
  .preview-empty svg { width: 36px; height: 36px; color: var(--text-muted,#999); }
  .playback { justify-content: center; padding: 8px; border: 1px solid var(--stroke-soft,var(--stroke,#ddd)); border-radius: 11px; background: var(--surface-subtle,var(--solid,#fff)); }
  .playback select { width: auto; }
  .counter { font-variant-numeric: tabular-nums; min-width: 52px; text-align: center; font-size: 12.5px; color: var(--text-secondary,#666); }
  .right { margin-left: auto; }
  progress { width: 100%; height: 6px; accent-color: var(--accent-strong,#222); }
  @media (max-width: 860px) { .stage { width: min(100%,500px); } }
  @media (max-width: 520px) { main { padding: 14px 12px max(96px,env(safe-area-inset-bottom)); } .bar { gap: 8px; } .bar select { flex-basis: 100%; } .bar button { flex: 1; } .card { padding: 15px; } .source { min-height: 190px; max-height: 285px; } .stage { min-height: 240px; } .playback { gap: 7px; } .playback select { flex: 1 1 100%; } }
  @media (max-width: 390px) { .pair { grid-template-columns: minmax(0,1fr); } .row > button { flex: 1; } .action .row > button { flex: none; } .upload-footer { align-items: flex-start; } .upload-button { padding-inline: 9px; } }
</style>
</head>
<body>
<main>
  <h1 class="sr-only" data-t="title"></h1>
  <div class="bar"><select id="project" aria-label="Project"></select><button id="new" type="button" data-t="new"></button><button id="refresh" type="button" data-t="refresh"></button></div>
  <p id="message" class="status" role="status" aria-live="polite"></p>
  <div class="columns">
    <div class="stack">
      <section class="card fields" aria-labelledby="source-title">
        <h2 id="source-title" data-step="01" data-t="source"></h2>
        <label><span data-t="name"></span><input id="name" maxlength="100" data-write /></label>
        <label id="upload-zone" class="upload-zone" for="upload" role="button" tabindex="0" data-testid="asset-upload-zone">
          <input id="upload" type="file" accept="image/png,image/jpeg,image/webp" aria-hidden="true" tabindex="-1" data-write />
          <span class="source"><img id="source-image" alt="" hidden /><span id="source-empty" class="source-empty"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3" y="4" width="18" height="16" rx="2"/><circle cx="8.5" cy="9" r="1.5"/><path d="m4 17 5-5 3 3 3-4 5 6"/></svg><strong data-t="upload"></strong><span class="muted" data-t="imageHint"></span></span></span>
          <span id="upload-footer" class="upload-footer" hidden><span class="upload-file"><strong id="upload-file-name"></strong><span id="upload-file-meta" class="muted"></span></span><span id="upload-title" class="upload-button" data-t="replaceImage"></span></span>
        </label>
        <label><span data-t="backgroundMode"></span><select id="background-mode" data-write aria-describedby="background-hint color-warning"><option value="model" data-t="modelMask" selected></option><option value="color" data-t="color"></option></select></label>
        <p id="background-hint" class="muted" data-t="backgroundHint"></p>
        <p id="color-warning" class="status" data-t="colorWarning" hidden></p>
        <div id="background-engine" class="fields" aria-live="polite"></div>
        <div class="row"><button id="background" type="button" data-t="background" data-write aria-describedby="background-engine background-hint"></button><button id="restore" type="button" data-t="restore" data-write hidden></button></div>
        <details><summary data-t="localTools"></summary><div class="fields">
          <p class="muted" data-t="localHint"></p>
          <details><summary data-t="inpaint"></summary><div class="fields">
            <p class="muted" data-t="regionHint"></p>
            <div class="pair"><label><span data-t="regionX"></span><input id="region-x" type="number" min="0" max="99" value="25" data-write /></label><label><span data-t="regionY"></span><input id="region-y" type="number" min="0" max="99" value="25" data-write /></label><label><span data-t="regionWidth"></span><input id="region-width" type="number" min="1" max="100" value="20" data-write /></label><label><span data-t="regionHeight"></span><input id="region-height" type="number" min="1" max="100" value="10" data-write /></label></div>
            <div id="inpaint-engine" class="fields"></div><button id="inpaint" type="button" data-t="applyInpaint" data-write></button>
          </div></details>
        </div></details>
      </section>
      <section class="card fields" aria-labelledby="generation-title">
        <h2 id="generation-title" data-step="02" data-t="generation"></h2>
        <label><span data-t="prompt"></span><textarea id="prompt" maxlength="8000" data-write></textarea></label>
        <div class="pair"><label><span data-t="model"></span><select id="model" data-write></select></label><label><span data-t="frameCount"></span><input id="frames" type="number" min="1" max="16" value="4" data-write /></label></div>
        <fieldset><legend data-t="directions"></legend><div id="directions" class="directions"></div></fieldset>
        <fieldset><legend data-t="actions"></legend><div id="actions" class="fields"></div></fieldset>
        <button id="add-action" type="button" data-t="addAction" data-write></button>
        <p id="request-count" class="muted"></p>
        <div class="row"><button id="save" type="button" data-t="save" data-write></button><button id="run" type="button" class="primary" data-t="run" data-write></button><button id="stop" type="button" data-t="stop" hidden></button></div>
      </section>
    </div>
    <section class="card fields" aria-labelledby="preview-title">
      <h2 id="preview-title" data-step="03" data-t="preview"></h2>
      <p id="run-status" class="status" role="status" aria-live="polite"></p>
      <div class="pair"><label><span data-t="action"></span><select id="preview-action"></select></label><label><span data-t="direction"></span><select id="preview-direction"></select></label></div>
      <div class="stage"><canvas id="preview" width="512" height="512" hidden></canvas><div id="preview-empty" class="preview-empty"><svg viewBox="0 0 40 40" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="4" y="7" width="32" height="26" rx="4"/><path d="m9 27 7-8 5 5 4-5 6 8"/><circle cx="14" cy="14" r="2"/></svg><p data-t="previewEmpty"></p></div></div>
      <div class="row playback"><button id="previous" type="button" data-t="previous"></button><button id="play" type="button" data-t="play"></button><button id="next" type="button" data-t="next"></button><span id="counter" class="counter">0 / 0</span><select id="fps" aria-label="Playback rate"><option value="0" data-t="originalRate"></option><option value="8">8 FPS</option><option value="12">12 FPS</option><option value="24">24 FPS</option></select></div>
      <div id="frame-editor"></div>
      <div class="row"><button id="export-png" type="button" data-t="exportPng"></button><button id="export-json" type="button" data-t="exportJson"></button></div>
    </section>
  </div>
</main>
<script src="/api/custom-ui/bridge.js"></script>
<script src="frame-editor.js"></script>
<script>
(function () {
  'use strict'
  var dictionaries = {
    en: { title:'Game Asset Workbench', subtitle:'Generate and refine game assets in independent projects.', new:'New project', refresh:'Refresh', select:'Choose a project', unsaved:'New · unsaved', source:'Character image', name:'Project name', imageHint:'Upload a PNG, JPEG or WebP character image, up to 8 MB.', upload:'Choose an image', replaceImage:'Replace image', background:'Remove background locally', restore:'Restore original image', localTools:'Local image tools', localHint:'Runs locally. Algorithms are downloaded only when you request them. Background removal never calls an image generation model.', backgroundMode:'Background removal', color:'Solid background (remove by color)', backgroundHint:'Local foreground detection is the default for complex backgrounds. Every attempt starts from the uploaded original and never calls a paid image model.', colorWarning:'Use color removal only for a solid background. Matching colors in the character may also be removed.', modelMask:'Local foreground detection · U²-Net', inpaint:'Repair a region', regionHint:'Rectangle coordinates are percentages of the original image. Use this to repair a small marked area.', regionX:'Left (%)', regionY:'Top (%)', regionWidth:'Width (%)', regionHeight:'Height (%)', applyInpaint:'Repair selected region', download:'Download algorithm', cancelDownload:'Cancel download', installed:'Algorithm ready · works offline', missing:'This local algorithm is not installed. Use the download button below; downloads start only when you click.', downloadFailed:'The algorithm could not be downloaded. Retry when the connection is available.', generation:'Animation settings', prompt:'Style and motion guidance', model:'Image model', noModels:'Configure an image model in settings', defaultModel:'Use default model', frameCount:'Frames per direction', directions:'Directions', actions:'Actions', addAction:'Add action', actionName:'Action name', actionPrompt:'Motion description', enabled:'Include action', removeAction:'Remove action', newAction:'Custom action', save:'Save project', run:'Save and generate', stop:'Stop generation', preview:'Animation preview', action:'Action', direction:'Direction', previewEmpty:'Generate assets to preview its animation frames here.', previous:'Previous', play:'Play', pause:'Pause', next:'Next', originalRate:'Frame timing', layoutPreview:'Layout preview · open this component as a tool to use projects.', frameLimit:'Choose no more than 512 frames in total across enabled actions and directions.', exportPng:'Export PNG atlas', exportJson:'Export frame data', idle:'Idle breathing', walk:'Walk', runAction:'Run', attack:'Attack', idlePrompt:'A subtle looping idle breathing animation. Keep the character anchored.', walkPrompt:'A smooth looping walk cycle with alternating steps and natural arm movement.', runPrompt:'A looping running cycle with clear stride and consistent character proportions.', attackPrompt:'An attack with anticipation, impact and recovery to the initial pose.', requestCount:'{actions} actions × {directions} directions = {count} image requests. Start with one direction to tune the result.', saved:'Project saved.', processed:'Image updated. Save the project to keep this image.', working:'Working…', loading:'Loading projects…', error:'The operation failed. Please retry.', loadError:'Could not load projects. Use Refresh to retry.', invalid:'Add a name, a character image, at least one action and one direction. Frame count must be 1–16.', imageInvalid:'Choose one PNG, JPEG or WebP image no larger than 8 MB.', dirty:'Unsaved changes', running:'Generating', waiting_approval:'Waiting for approval', completed:'Completed', failed:'Generation failed. Check the image model settings.', cancelled:'Stopped', interrupted:'Interrupted', queued:'Queued', regionInvalid:'Use a rectangle inside the image, with width and height greater than zero.', S:'South', SW:'Southwest', W:'West', NW:'Northwest', N:'North', NE:'Northeast', E:'East', SE:'Southeast' },
    zh: { title:'游戏素材工作台', subtitle:'独立管理游戏素材，生成动作并逐帧调整。', new:'新建项目', refresh:'刷新', select:'选择项目', unsaved:'新项目 · 未保存', source:'角色图片', name:'项目名称', imageHint:'上传 PNG、JPEG 或 WebP 角色图片，最大 8 MB。', upload:'选择图片', replaceImage:'更换图片', background:'本地去背景', restore:'恢复原图', localTools:'本地图像工具', localHint:'全部在本地处理，算法仅在点击下载时获取。去背景不会调用图像生成模型。', backgroundMode:'去背景方式', color:'纯色背景（按颜色移除）', backgroundHint:'默认在本地识别复杂背景中的前景。每次基于上传原图重新处理，不调用付费图像模型。', colorWarning:'按颜色移除仅适合纯色背景。主体与背景同色时，可能误删主体。', modelMask:'本地前景识别 · U²-Net', inpaint:'区域修补', regionHint:'区域按原图的百分比填写，适合修补小范围标记。', regionX:'左侧位置（%）', regionY:'顶部位置（%）', regionWidth:'宽度（%）', regionHeight:'高度（%）', applyInpaint:'修补选定区域', download:'下载算法', cancelDownload:'取消下载', installed:'算法已就绪 · 可离线使用', missing:'尚未安装此本地算法，请先点击下方按钮下载；不会自动下载。', downloadFailed:'算法下载失败，请在网络可用后重试。', generation:'动画设置', prompt:'风格与动作要求', model:'图像模型', noModels:'请先在设置中配置图像模型', defaultModel:'使用默认模型', frameCount:'每个方向的帧数', directions:'生成方向', actions:'动作', addAction:'添加动作', actionName:'动作名称', actionPrompt:'动作描述', enabled:'启用此动作', removeAction:'删除动作', newAction:'自定义动作', save:'保存项目', run:'保存并生成', stop:'停止生成', preview:'动画预览', action:'动作', direction:'方向', previewEmpty:'运行项目后，在这里查看并播放动画帧。', previous:'上一帧', play:'播放', pause:'暂停', next:'下一帧', originalRate:'按帧时长播放', layoutPreview:'布局预览，打开工具后即可使用。', frameLimit:'所有启用动作与方向的总帧数不能超过 512。', exportPng:'导出 PNG 图集', exportJson:'导出帧信息', idle:'待机呼吸', walk:'走路', runAction:'跑步', attack:'攻击', idlePrompt:'轻微、循环的待机呼吸动作，保持角色位置稳定。', walkPrompt:'平滑循环的步行动作，双腿交替迈步，手臂自然摆动。', runPrompt:'循环的跑步动作，步幅清晰，保持角色比例一致。', attackPrompt:'攻击动作包含预备、发力和收势，最后回到初始姿态。', requestCount:'{actions} 个动作 × {directions} 个方向 = {count} 次图像请求。建议先选一个方向调整效果。', saved:'项目已保存。', processed:'图片已更新，保存项目即可保留。', working:'正在处理…', loading:'正在加载项目…', error:'操作失败，请重试。', loadError:'项目加载失败，请点击刷新重试。', invalid:'请填写名称、上传角色图片，并至少选择一个动作与方向。帧数需要在 1–16 之间。', imageInvalid:'请选择一张不超过 8 MB 的 PNG、JPEG 或 WebP 图片。', dirty:'有未保存的修改', running:'正在生成', waiting_approval:'等待审批', completed:'已完成', failed:'生成失败，请检查图像模型设置。', cancelled:'已停止', interrupted:'运行中断', queued:'等待运行', regionInvalid:'请填写图片范围内的矩形区域，宽度与高度不能为零。', S:'南', SW:'西南', W:'西', NW:'西北', N:'北', NE:'东北', E:'东', SE:'东南' }
  }
  var locale = 'en', generation = 0, disposed = false, busy = false, polling = false, pollTimer = null
  var projects = [], jobs = [], models = [], engines = [], draft = null, original = null, dirty = false
  var sourceUrl = null, sourceRevision = 0, previewRevision = 0, outputKey = '', output = null, currentJob = null
  var frameIndex = 0, playing = false, playbackTimer = null, cache = new Map(), frameRequests = new Map()
  var editor = null
  var bridge = null, unsubscribe = null, engineBusy = new Set(), previewOnly = false
  var directionIds = ['S','SW','W','NW','N','NE','E','SE']
  function el(id) { return document.getElementById(id) }
  function words() { return dictionaries[/^zh/i.test(locale) ? 'zh' : 'en'] }
  function copy(value) { return JSON.parse(JSON.stringify(value)) }
  function message(key) { el('message').textContent = key ? words()[key] || words().error : '' }
  function option(value, name) { var node = document.createElement('option'); node.value = value; node.textContent = name; return node }
  function defaultDraft() { var w = words(); return { name: w.title, prompt:'', reference:null, frameCount:4, directions:directionIds.slice(), model:null, originalReference:null, actions:[{id:'idle',name:w.idle,prompt:w.idlePrompt,enabled:true},{id:'walk',name:w.walk,prompt:w.walkPrompt,enabled:true},{id:'run',name:w.runAction,prompt:w.runPrompt,enabled:true},{id:'attack',name:w.attack,prompt:w.attackPrompt,enabled:true}] } }
  function active(run) { return Boolean(run && ['running','queued','waiting_approval'].indexOf(run.status) >= 0) }
  function changed() { if (!draft) return; dirty = true; message('dirty'); renderCount(); renderLocks() }
  function renderCount() { if (!draft) return; var count = draft.actions.filter(function (a) { return a.enabled }).length, directions = draft.directions.length; el('request-count').textContent = words().requestCount.replace('{actions}',String(count)).replace('{directions}',String(directions)).replace('{count}',String(count * directions)) }
  function renderLocks() {
    if (previewOnly) { document.querySelectorAll('button,input,select,textarea').forEach(function (node) { node.disabled = true }); el('upload-zone').setAttribute('aria-disabled','true'); el('upload-zone').tabIndex = -1; return }
    var editing = Boolean(editor && editor.dirty())
    var locked = !bridge || busy || !draft || active(currentJob) || editing
    el('project').disabled = busy || editing
    el('new').disabled = busy || editing
    document.querySelectorAll('[data-write]').forEach(function (node) { node.disabled = locked })
    el('upload-zone').setAttribute('aria-disabled',String(locked))
    el('upload-zone').tabIndex = locked ? -1 : 0
    el('background-engine').hidden = el('background-mode').value !== 'model'
    el('color-warning').hidden = el('background-mode').value !== 'color'
    el('background').disabled = locked || !draft.reference || (el('background-mode').value === 'model' && !engineReady('background'))
    el('inpaint').disabled = locked || !draft.reference || !engineReady('inpaint')
    el('restore').hidden = !original || !draft.reference || original.id === draft.reference.id
    el('stop').hidden = !active(currentJob)
    el('stop').disabled = busy
    el('run').disabled = locked || models.length === 0
    el('add-action').disabled = locked || draft.actions.length >= 8
    var frames = visibleFrames()
    ;['play','previous','next','fps'].forEach(function (id) { el(id).disabled = !frames.length })
    el('export-png').disabled = busy || editing || !output || !output.atlas
    el('export-json').disabled = busy || editing || !output || !output.atlas
    if (editor) editor.setBusy(busy || active(currentJob) || previewOnly)
  }
  function renderProjectOptions() {
    var select = el('project'), selected = draft && draft.id || ''
    select.replaceChildren(option('',words().unsaved))
    projects.forEach(function (project) { select.append(option(project.id,project.name)) })
    select.value = selected
    select.setAttribute('aria-label',words().select)
  }
  function renderModels() {
    var select = el('model'), selected = draft && draft.model ? draft.model.provider + '/' + draft.model.model : ''
    select.replaceChildren(option('',models.length ? words().defaultModel : words().noModels))
    models.forEach(function (model) { select.append(option(model.id,(model.providerName ? model.providerName + ' · ' : '') + model.name)) })
    if (selected && !models.some(function (model) { return model.id === selected })) select.append(option(selected,selected))
    select.value = selected
  }
  function renderDirections() {
    var container = el('directions'); container.replaceChildren()
    directionIds.forEach(function (id) { var label = document.createElement('label'), input = document.createElement('input'), text = document.createElement('span'); input.type = 'checkbox'; input.checked = draft.directions.indexOf(id) >= 0; input.setAttribute('data-write',''); input.addEventListener('change',function () { draft.directions = directionIds.filter(function (direction) { return direction === id ? input.checked : draft.directions.indexOf(direction) >= 0 }); changed() }); text.textContent = words()[id]; label.append(input,text); container.append(label) })
  }
  function renderActions() {
    var container = el('actions'); container.replaceChildren()
    draft.actions.forEach(function (action,index) {
      var box = document.createElement('div'), row = document.createElement('div'), enabled = document.createElement('input'), name = document.createElement('input'), remove = document.createElement('button'), prompt = document.createElement('textarea')
      box.className = 'action'; row.className = 'row'; enabled.type = 'checkbox'; enabled.checked = action.enabled; enabled.setAttribute('aria-label',words().enabled + ' · ' + action.name); name.type = 'text'; name.value = action.name; name.maxLength = 100; name.setAttribute('aria-label',words().actionName); prompt.value = action.prompt; prompt.maxLength = 2000; prompt.setAttribute('aria-label',words().actionPrompt); remove.type = 'button'; remove.className = 'icon'; remove.textContent = '×'; remove.setAttribute('aria-label',words().removeAction + ' · ' + action.name)
      ;[enabled,name,remove,prompt].forEach(function (node) { node.setAttribute('data-write','') })
      enabled.addEventListener('change',function () { action.enabled = enabled.checked; changed() }); name.addEventListener('input',function () { action.name = name.value; changed() }); prompt.addEventListener('input',function () { action.prompt = prompt.value; changed() }); remove.addEventListener('click',function () { draft.actions.splice(index,1); renderActions(); changed(); el('add-action').focus() })
      row.append(enabled,name,remove); box.append(row,prompt); container.append(box)
    })
  }
  function renderUploadState() {
    var media = draft && (draft.originalReference || draft.reference), selected = Boolean(media)
    var size = selected ? media.size < 1024 ? new Intl.NumberFormat(locale).format(media.size) + ' B' : media.size < 1024*1024 ? new Intl.NumberFormat(locale,{maximumFractionDigits:1}).format(media.size/1024) + ' KB' : new Intl.NumberFormat(locale,{maximumFractionDigits:1}).format(media.size/1024/1024) + ' MB' : ''
    el('upload-zone').classList.toggle('has-image',selected)
    el('upload-zone').setAttribute('aria-label',selected ? words().replaceImage : words().upload)
    el('upload').setAttribute('aria-label',words().upload)
    el('upload-footer').hidden = !selected
    el('upload-file-name').textContent = selected ? media.name : ''
    el('upload-file-meta').textContent = selected ? media.mimeType.split('/')[1].toUpperCase() + ' · ' + size : ''
  }
  function replaceSourceUrl(value) { if (sourceUrl) URL.revokeObjectURL(sourceUrl); sourceUrl = value; el('source-image').hidden = !value; el('source-empty').hidden = Boolean(value); if (value) el('source-image').src = value; else el('source-image').removeAttribute('src') }
  async function renderSource() {
    var revision = ++sourceRevision, selected = generation
    renderUploadState()
    replaceSourceUrl(null)
    if (!draft.reference || !bridge) return
    try { var data = await bridge.image({id:draft.reference.id}); if (disposed || revision !== sourceRevision || selected !== generation) return; replaceSourceUrl(URL.createObjectURL(new Blob([data.buffer],{type:data.mimeType}))) } catch { if (!disposed && revision === sourceRevision && selected === generation) message('error') }
  }
  function selectProject(project) {
    generation++; el('background-mode').value = 'model'; busy = false; dirty = false; draft = project ? copy(project) : defaultDraft(); original = draft.originalReference ? copy(draft.originalReference) : null; currentJob = null; output = null; outputKey = ''; pause(); clearCache(); el('name').value = draft.name; el('prompt').value = draft.prompt; el('frames').value = String(draft.frameCount); renderProjectOptions(); renderModels(); renderDirections(); renderActions(); renderSource(); renderCount(); renderResults(); renderLocks(); message('')
  }
  function engineReady(id) { return engines.some(function (engine) { return engine.id === id && engine.status === 'ready' }) }
  function renderEngines() {
    ;['background','inpaint'].forEach(function (id) {
      var box = el(id + '-engine'), engine = engines.find(function (entry) { return entry.id === id }), text = document.createElement('p')
      var size = engine && engine.bytes ? new Intl.NumberFormat(locale,{maximumFractionDigits:1}).format(engine.bytes/1024/1024) + ' MB' : ''; box.replaceChildren(); text.className = 'muted'; var downloading = engine && engine.status === 'downloading'; text.textContent = engineReady(id) ? words().installed : engine && engine.error ? words().downloadFailed : words().missing; if (engine && engine.name) text.textContent = engine.name + (size ? ' · ' + size : '') + ' · ' + text.textContent; box.append(text)
      if (!engineReady(id)) {
        var button = document.createElement('button'); button.type = 'button'; button.textContent = downloading ? words().cancelDownload : words().download + (size ? ' · ' + size : ''); button.disabled = engineBusy.has(id)
        button.addEventListener('click',async function () { if (engineBusy.has(id) || !bridge) return; engineBusy.add(id); renderEngines(); try { await bridge.engine({id:id,action:downloading?'cancel':'download'}); await refresh() } catch { if (!disposed) message('error') } finally { engineBusy.delete(id); if (!disposed) renderEngines() } }); box.append(button)
        if (downloading) { var progress = document.createElement('progress'); if (engine.total > 0) { progress.max = engine.total; progress.value = engine.received || 0 } progress.setAttribute('aria-label',words().download); box.append(progress) }
      }
    })
    renderLocks()
  }
  function clearCache() { previewRevision++; cache.forEach(function (item) { URL.revokeObjectURL(item.url) }); cache.clear(); frameRequests.clear() }
  function visibleFrames() { if (!output || !Array.isArray(output.frames)) return []; return output.frames.filter(function (frame) { return frame.action === el('preview-action').value && frame.direction === el('preview-direction').value }) }
  function renderPreviewChoices() {
    var action = el('preview-action'), direction = el('preview-direction'), oldAction = action.value, oldDirection = direction.value, frames = output && Array.isArray(output.frames) ? output.frames : []
    var actions = Array.from(new Set(frames.map(function (frame) { return frame.action })))
    action.replaceChildren(); actions.forEach(function (name) { action.append(option(name,name || words().action)) }); if (actions.indexOf(oldAction) >= 0) action.value = oldAction
    var directions = Array.from(new Set(frames.filter(function (frame) { return frame.action === action.value }).map(function (frame) { return frame.direction })))
    direction.replaceChildren(); directions.forEach(function (name) { direction.append(option(name,words()[name] || name || words().direction)) }); if (directions.indexOf(oldDirection) >= 0) direction.value = oldDirection
  }
  async function frameImage(media) {
    if (cache.has(media.id)) { var existing = cache.get(media.id); cache.delete(media.id); cache.set(media.id,existing); return existing.image }
    if (frameRequests.has(media.id)) return frameRequests.get(media.id)
    var epoch = previewRevision
    var request = (async function () {
      var data = await bridge.image({id:media.id}); if (disposed || epoch !== previewRevision) return null
      var url = URL.createObjectURL(new Blob([data.buffer],{type:data.mimeType})), image = new Image()
      try { await new Promise(function (resolve,reject) { image.onload = resolve; image.onerror = reject; image.src = url }); if (disposed || epoch !== previewRevision) { URL.revokeObjectURL(url); return null } cache.set(media.id,{url:url,image:image}); while (cache.size > 4) { var first = cache.keys().next().value; URL.revokeObjectURL(cache.get(first).url); cache.delete(first) } return image } catch (error) { URL.revokeObjectURL(url); throw error }
    })()
    frameRequests.set(media.id,request)
    try { return await request } finally { if (frameRequests.get(media.id) === request) frameRequests.delete(media.id) }
  }
  async function drawFrame() {
    var frames = visibleFrames(), revision = previewRevision, selected = generation
    frameIndex = frames.length ? (frameIndex + frames.length) % frames.length : 0
    var index = frameIndex, canvas = el('preview'); el('counter').textContent = (frames.length ? frameIndex + 1 : 0) + ' / ' + frames.length; canvas.hidden = !frames.length; el('preview-empty').hidden = Boolean(frames.length)
    if (!frames.length) return
    try { var image = await frameImage(frames[index].media); if (!image || disposed || selected !== generation || revision !== previewRevision || index !== frameIndex) return; var context = canvas.getContext('2d'); context.clearRect(0,0,canvas.width,canvas.height); var scale = Math.min(canvas.width/image.naturalWidth,canvas.height/image.naturalHeight); context.imageSmoothingEnabled = false; context.drawImage(image,(canvas.width-image.naturalWidth*scale)/2,(canvas.height-image.naturalHeight*scale)/2,image.naturalWidth*scale,image.naturalHeight*scale) } catch { if (!disposed && selected === generation && revision === previewRevision) { pause(); message('error') } }
  }
  function pause() { playing = false; if (playbackTimer !== null) clearTimeout(playbackTimer); playbackTimer = null; el('play').textContent = words().play }
  function scheduleFrame() {
    if (!playing || disposed || document.hidden) return
    var frames = visibleFrames(); if (!frames.length) { pause(); return }
    var fps = Number(el('fps').value), delay = fps > 0 ? 1000/fps : Number(frames[frameIndex].durationMs) || 125
    playbackTimer = setTimeout(async function () { if (!playing || disposed) return; frameIndex = (frameIndex + 1) % frames.length; await drawFrame(); scheduleFrame() },Math.max(16,delay))
  }
  function renderResults() {
    var matching = draft && draft.id ? jobs.filter(function (run) { return run.projectId === draft.id }).sort(function (a,b) { return String(b.startedAt || '').localeCompare(String(a.startedAt || '')) }) : []
    currentJob = matching[0] || null
    el('run-status').textContent = currentJob ? words()[currentJob.status] || currentJob.status : ''
    var result = currentJob && currentJob.output && currentJob.output.frames.length ? currentJob.output : null
    var key = result ? currentJob.id + ':' + (result.atlas ? result.atlas.media.id : result.frames.map(function (frame) { return frame.media.id }).join(',')) : ''
    if (key !== outputKey || !key) { outputKey = key; output = result; pause(); clearCache(); frameIndex = 0; renderPreviewChoices(); drawFrame() }
    if (editor) editor.setJob(currentJob)
    renderLocks()
  }
  async function refresh() {
    if (polling || disposed || !bridge) return
    polling = true
    try { var data = await bridge.list(); if (disposed) return; projects = Array.isArray(data.projects) ? data.projects : []; var localJobs = new Map(jobs.map(function (job) { return [job.id,job] })); jobs = (Array.isArray(data.jobs) ? data.jobs : []).map(function (job) { var local = localJobs.get(job.id); return local && local.revision > job.revision ? local : job }); models = Array.isArray(data.models) ? data.models : []; engines = Array.isArray(data.engines) ? data.engines : []; renderProjectOptions(); renderModels(); renderResults(); renderEngines() } catch { if (!disposed) message('loadError') } finally { polling = false }
  }
  async function mutate(operation, success) {
    if (busy || !bridge || !draft || active(currentJob)) return
    var selected = generation; busy = true; message('working'); renderLocks()
    try { var result = await operation(); if (disposed || generation !== selected) return; if (success) success(result); message('processed') } catch { if (!disposed && selected === generation) message('error') } finally { if (!disposed && generation === selected) { busy = false; renderLocks() } }
  }
  async function save(andRun) {
    if (busy || !bridge || !draft || active(currentJob)) return
    if (!draft.name.trim() || (andRun && !draft.reference) || !Number.isInteger(draft.frameCount) || draft.frameCount < 1 || draft.frameCount > 16 || !draft.directions.length || !draft.actions.some(function (action) { return action.enabled && action.name.trim() && action.prompt.trim() })) { message('invalid'); return }
    if (draft.actions.filter(function (action) { return action.enabled }).length * draft.directions.length * draft.frameCount > 512) { message('frameLimit'); return }
    var selected = generation, payload = copy(draft); delete payload.createdAt; delete payload.updatedAt; busy = true; message('working'); renderLocks()
    try {
      var saved = await bridge.save(payload); if (disposed || selected !== generation) return
      draft = copy(saved); dirty = false; var index = projects.findIndex(function (project) { return project.id === saved.id }); if (index >= 0) projects[index] = saved; else projects.unshift(saved); renderProjectOptions(); message('saved')
      if (andRun) { var result = await bridge.run({id:saved.id}); if (disposed || selected !== generation) return; if (result && result.job) jobs.unshift(result.job); renderResults() }
      await refresh()
    } catch { if (!disposed && generation === selected) message('error') } finally { if (!disposed && generation === selected) { busy = false; renderLocks() } }
  }
  function localize(value) {
    if (typeof value === 'string' && value) locale = value
    document.documentElement.lang = locale
    if (editor) editor.setLocale(locale)
    document.querySelectorAll('[data-t]').forEach(function (node) { node.textContent = words()[node.dataset.t] || '' })
    el('source-image').alt = words().source; el('fps').setAttribute('aria-label',words().originalRate)
    renderUploadState()
    if (draft) { renderProjectOptions(); renderModels(); renderDirections(); renderActions(); renderPreviewChoices(); renderCount(); renderResults(); renderEngines(); renderLocks(); if (previewOnly) message('layoutPreview') }
  }
  el('project').addEventListener('change',function () { selectProject(projects.find(function (project) { return project.id === el('project').value })) })
  el('new').addEventListener('click',function () { selectProject(null); el('name').focus() })
  el('refresh').addEventListener('click',function () { refresh() })
  ;['name','prompt','frames'].forEach(function (id) { el(id).addEventListener('input',function () { if (!draft) return; draft[id === 'frames' ? 'frameCount' : id] = id === 'frames' ? Number(el(id).value) : el(id).value; changed() }) })
  el('model').addEventListener('change',function () { var model = models.find(function (item) { return item.id === el('model').value }); draft.model = model ? {provider:model.providerId,model:model.id.slice(model.providerId.length + 1)} : null; changed() })
  el('background-mode').addEventListener('change',renderLocks)
  el('upload-zone').addEventListener('keydown',function (event) { if (event.target !== el('upload-zone') || el('upload').disabled) return; if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); el('upload').click() } })
  el('add-action').addEventListener('click',function () { if (!draft || busy || draft.actions.length >= 8) return; draft.actions.push({id:'action-' + Date.now().toString(36) + '-' + Math.random().toString(36).slice(2,7),name:words().newAction,prompt:'',enabled:true}); renderActions(); changed(); el('actions').lastElementChild.querySelector('input[type=text]').focus() })
  el('upload').addEventListener('change',function () {
    var file = el('upload').files && el('upload').files[0]; el('upload').value = ''; if (!file) return
    if (['image/png','image/jpeg','image/webp'].indexOf(file.type) < 0 || !file.size || file.size > 8*1024*1024) { message('imageInvalid'); return }
    mutate(async function () { return bridge.uploadImage({name:file.name,mimeType:file.type,buffer:await file.arrayBuffer()}) },function (media) { el('background-mode').value = 'model'; draft.reference = media; original = copy(media); draft.originalReference = copy(media); dirty = true; renderSource() })
  })
  el('background').addEventListener('click',function () { if (!draft || !draft.reference) return; var method = el('background-mode').value; if (method === 'model' && !engineReady('background')) { message('missing'); return } var reference = copy(draft.originalReference || draft.reference); mutate(function () { return bridge.process({reference:reference,operation:'background',image:{method:method}}) },function (media) { draft.reference = media; dirty = true; renderSource() }) })
  el('restore').addEventListener('click',function () { if (!original || busy) return; draft.reference = copy(original); renderSource(); changed() })
  el('inpaint').addEventListener('click',function () {
    if (!draft || !draft.reference) return
    var region = {x:Number(el('region-x').value),y:Number(el('region-y').value),width:Number(el('region-width').value),height:Number(el('region-height').value)}
    if (Object.values(region).some(function (value) { return !Number.isFinite(value) }) || region.x < 0 || region.y < 0 || region.width <= 0 || region.height <= 0 || region.x + region.width > 100 || region.y + region.height > 100) { message('regionInvalid'); return }
    var reference = copy(draft.reference); mutate(function () { return bridge.process({reference:reference,operation:'inpaint',image:{region:region}}) },function (media) { draft.reference = media; dirty = true; renderSource() })
  })
  el('save').addEventListener('click',function () { save(false) }); el('run').addEventListener('click',function () { save(true) })
  el('stop').addEventListener('click',async function () { if (busy || !currentJob) return; var selected = generation, id = currentJob.id; busy = true; renderLocks(); try { await bridge.stop({jobId:id}); await refresh() } catch { if (selected === generation) message('error') } finally { if (selected === generation) { busy = false; renderLocks() } } })
  el('preview-action').addEventListener('change',function () { pause(); clearCache(); frameIndex = 0; renderPreviewChoices(); drawFrame(); renderLocks() })
  el('preview-direction').addEventListener('change',function () { pause(); clearCache(); frameIndex = 0; drawFrame(); renderLocks() })
  el('previous').addEventListener('click',function () { pause(); frameIndex--; drawFrame() }); el('next').addEventListener('click',function () { pause(); frameIndex++; drawFrame() })
  el('play').addEventListener('click',function () { if (playing) pause(); else if (visibleFrames().length) { playing = true; el('play').textContent = words().pause; scheduleFrame() } })
  el('fps').addEventListener('change',function () { if (playing) { clearTimeout(playbackTimer); scheduleFrame() } })
  ;['png','json'].forEach(function (format) { el('export-' + format).addEventListener('click',async function () { if (!currentJob || !output || busy) return; var selected = generation; busy = true; renderLocks(); try { await bridge.export({jobId:currentJob.id,format:format}) } catch { if (selected === generation) message('error') } finally { if (selected === generation) { busy = false; renderLocks() } } }) })
  document.addEventListener('visibilitychange',function () { if (document.hidden) pause(); else refresh() })
  window.addEventListener('pagehide',function () { disposed = true; generation++; sourceRevision++; clearInterval(pollTimer); pause(); clearCache(); replaceSourceUrl(null); if (unsubscribe) unsubscribe(); if (editor) editor.dispose() })
  editor = window.createGameAssetFrameEditor({root:el('frame-editor'), image:frameImage, onDirty:renderLocks, save:async function (jobId,edits) { var selected = generation; busy = true; renderLocks(); try { var job = await bridge.editFrames({jobId:jobId,edits:edits}); if (disposed || selected !== generation) return null; var index = jobs.findIndex(function (item) { return item.id === job.id }); if (index >= 0) jobs[index] = job; else jobs.unshift(job); renderResults(); return job } finally { if (!disposed && selected === generation) { busy = false; renderLocks() } } }})
  localize(locale); selectProject(null); message('loading')
  if (window.pisper) {
    window.pisper.ready().then(async function (ready) { if (disposed) return; localize(ready && (ready.locale || ready.theme && ready.theme.locale)); var permissions = ready && ready.component && ready.component.permissions; if (Array.isArray(permissions) && permissions.indexOf('game-assets.read') < 0) { previewOnly = true; selectProject(null); message('layoutPreview'); unsubscribe = window.pisper.onThemeChanged(function (theme) { localize(theme && theme.locale) }); return } bridge = window.pisper.gameAssets; if (!bridge) { message('loadError'); return } selectProject(null); unsubscribe = window.pisper.onThemeChanged(function (theme) { localize(theme && theme.locale) }); var initialGeneration = generation; await refresh(); if (disposed) return; if (generation === initialGeneration && !dirty) { if (projects.length) selectProject(projects[0]); else message('') } pollTimer = setInterval(function () { if (!document.hidden) refresh() },1500) }).catch(function () { if (!disposed) message('loadError') })
  } else message('loadError')
})()
</script>
</body>
</html>`,
  }),
})
