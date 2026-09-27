// iframe 内的独立帧草稿编辑器。所有改动显式应用后才写入项目；图片与持久化由宿主桥负责。
import { applyImageAlphaStrokes } from '../../shared/image-alpha-strokes.mjs'

// 输出的是静态函数声明，浏览器正常解析脚本；不使用 eval，也不依赖序列化闭包。
export const GAME_ASSET_FRAME_EDITOR_SCRIPT = String.raw`
(function () {
  'use strict'
  var applyImageAlphaStrokes = ${applyImageAlphaStrokes.toString()}
  var text = {
    zh: {title:'逐帧编辑',hint:'拖动调整位置；擦除与恢复只改变当前帧。应用修改会在本地重建图集，不调用 AI。',empty:'生成完成后可手动调整每一帧。',move:'移动',erase:'擦除',restore:'恢复',brush:'笔刷半径（%）',onion:'洋葱皮',x:'水平偏移',y:'垂直偏移',scale:'缩放',rotation:'旋转（°）',opacity:'透明度',durationMs:'帧时长（毫秒）',earlier:'向前移动',later:'向后移动',duplicate:'复制帧',remove:'删除帧',undo:'撤销',redo:'重做',reset:'重置为原始帧',discard:'放弃未保存修改',apply:'应用帧修改',dirty:'有未保存的帧修改；应用或放弃后可切换项目和重新生成。',saved:'帧修改已保存。',saving:'正在本地应用修改…',error:'帧修改失败，请重试。',limit:'已达到帧或笔刷编辑上限。',frame:'帧',previous:'上一组',next:'下一组'},
    en: {title:'Edit individual frames',hint:'Drag to position the frame. Erase and restore affect only this frame. Applying edits rebuilds the atlas locally without AI.',empty:'Generate assets to edit their individual frames.',move:'Move',erase:'Erase',restore:'Restore',brush:'Brush radius (%)',onion:'Onion skin',x:'Horizontal offset',y:'Vertical offset',scale:'Scale',rotation:'Rotation (°)',opacity:'Opacity',durationMs:'Frame duration (ms)',earlier:'Move earlier',later:'Move later',duplicate:'Duplicate frame',remove:'Delete frame',undo:'Undo',redo:'Redo',reset:'Reset to original frames',discard:'Discard unsaved edits',apply:'Apply frame edits',dirty:'Unsaved frame edits. Apply or discard before switching projects or generating again.',saved:'Frame edits saved.',saving:'Applying edits locally…',error:'Frame edits failed. Please retry.',limit:'Frame or brush editing limit reached.',frame:'Frame',previous:'Previous group',next:'Next group'}
  }
  var styles = document.createElement('style')
  styles.textContent = '.frame-editor{min-width:0}.frame-editor .edit-strip{display:flex;gap:6px;overflow:auto;padding:3px}.frame-editor .edit-thumb{flex:0 0 58px;padding:3px;font-size:11px}.frame-editor .edit-thumb[aria-pressed=true]{outline:2px solid var(--focus,#777);outline-offset:-2px}.frame-editor .edit-thumb canvas{display:block;width:48px;height:48px;object-fit:contain}.frame-editor .edit-stage{aspect-ratio:1;position:relative;display:grid;place-items:center;background:repeating-conic-gradient(#8882 0% 25%,transparent 0% 50%) 0 0/20px 20px;border:1px solid var(--stroke,#ddd);border-radius:10px;overflow:hidden}.frame-editor .edit-stage canvas{display:block;max-width:100%;max-height:100%;touch-action:none;cursor:move;image-rendering:pixelated}.frame-editor .edit-fields{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:9px}.frame-editor .edit-fields label{font-size:12px}.frame-editor .edit-controls{display:flex;flex-wrap:wrap;gap:6px}.frame-editor .edit-controls label{display:flex;align-items:center;gap:6px}.frame-editor .edit-controls button{font-size:12px;padding:6px 9px}.frame-editor .edit-info{font-variant-numeric:tabular-nums;color:var(--text-secondary,#666);font-size:12px}'
  document.head.append(styles)
  window.createGameAssetFrameEditor = function (options) {
    var root=options.root, locale='en', job=null, key='', frames=[], saved=[], undo=[], redo=[], selected=0, busy=false, applying=false, disposed=false, epoch=0, gesture=null, tool='move', pageStart=0
    root.className='frame-editor'
    root.innerHTML='<details><summary data-e="title"></summary><div class="fields"><p class="muted" data-e="hint"></p><p class="muted" id="edit-empty" data-e="empty"></p><div id="edit-content" class="fields" hidden><div class="edit-controls"><button type="button" id="edit-page-previous" data-e="previous"></button><span id="edit-info" class="edit-info"></span><button type="button" id="edit-page-next" data-e="next"></button></div><div id="edit-strip" class="edit-strip" role="group"></div><div class="edit-stage"><canvas id="edit-canvas" width="256" height="256" tabindex="0"></canvas></div><div class="edit-controls"><button type="button" data-tool="move" data-e="move"></button><button type="button" data-tool="erase" data-e="erase"></button><button type="button" data-tool="restore" data-e="restore"></button><label><input id="edit-onion" type="checkbox"/><span data-e="onion"></span></label></div><label><span data-e="brush"></span><input id="edit-brush" type="range" min="0.1" max="25" step="0.1" value="3"/></label><div id="edit-fields" class="edit-fields"></div><div class="edit-controls"><button type="button" id="edit-earlier" data-e="earlier"></button><button type="button" id="edit-later" data-e="later"></button><button type="button" id="edit-duplicate" data-e="duplicate"></button><button type="button" id="edit-remove" data-e="remove"></button></div><div class="edit-controls"><button type="button" id="edit-undo" data-e="undo"></button><button type="button" id="edit-redo" data-e="redo"></button><button type="button" id="edit-reset" data-e="reset"></button><button type="button" id="edit-discard" data-e="discard"></button></div><button type="button" id="edit-apply" class="primary" data-e="apply"></button></div><p id="edit-status" class="status" role="status" aria-live="polite"></p></div></details>'
    function el(id){return root.querySelector('#edit-'+id)}
    function w(){return text[/^zh/i.test(locale)?'zh':'en']}
    function copy(value){return JSON.parse(JSON.stringify(value))}
    function dirty(){return JSON.stringify(frames)!==JSON.stringify(saved)}
    function status(name){el('status').textContent=name?w()[name]:''}
    function jobKey(value){return value?value.id+':'+value.revision+':'+value.status+':'+(value.originalOutput?value.originalOutput.frames.length:0):''}
    function base(){return job&&job.originalOutput?job.originalOutput.frames:[]}
    function original(){return base().map(function(frame,index){return{sourceIndex:index,x:0,y:0,rotation:0,scale:1,opacity:1,durationMs:frame.durationMs,eraseStrokes:[]}})}
    function snapshot(){return {frames:copy(frames),selected:selected}}
    function restore(value){frames=copy(value.frames);selected=Math.min(value.selected,frames.length-1);pageStart=Math.floor(selected/8)*8;render();options.onDirty()}
    function remember(value){undo.push(value||snapshot());if(undo.length>20)undo.shift();redo=[]}
    function changed(){status(dirty()?'dirty':'');render();options.onDirty()}
    function source(edit){return base()[edit.sourceIndex]}
    var ranges={x:[-4096,4096,1],y:[-4096,4096,1],scale:[0.05,8,0.05],rotation:[-360,360,1],opacity:[0,1,0.05],durationMs:[16,10000,1]}
    Object.keys(ranges).forEach(function(name){var label=document.createElement('label'),span=document.createElement('span'),input=document.createElement('input');span.dataset.e=name;input.type='number';input.id='edit-'+name;input.min=ranges[name][0];input.max=ranges[name][1];input.step=ranges[name][2];label.append(span,input);el('fields').append(label);input.addEventListener('change',function(){var value=Number(input.value);if(busy||applying||!frames[selected]||!Number.isFinite(value)||value<ranges[name][0]||value>ranges[name][1]||(name==='durationMs'&&!Number.isInteger(value))){render();return}remember();frames[selected][name]=value;changed()})})
    function localize(){root.querySelectorAll('[data-e]').forEach(function(node){node.textContent=w()[node.dataset.e]});el('canvas').setAttribute('aria-label',w().title);el('strip').setAttribute('aria-label',w().frame);render()}
    function render(){
      if(disposed)return
      var exists=frames.length>0,locked=busy||applying
      el('empty').hidden=exists;el('content').hidden=!exists
      root.querySelectorAll('button,input').forEach(function(node){node.disabled=locked||!exists})
      root.querySelectorAll('[data-tool]').forEach(function(node){node.setAttribute('aria-pressed',String(node.dataset.tool===tool))})
      el('undo').disabled=locked||!undo.length;el('redo').disabled=locked||!redo.length;el('discard').disabled=locked||!dirty();el('apply').disabled=locked||!dirty();el('earlier').disabled=locked||selected<=0;el('later').disabled=locked||selected>=frames.length-1;el('duplicate').disabled=locked||frames.length>=512;el('remove').disabled=locked||frames.length<=1
      el('page-previous').disabled=locked||pageStart===0;el('page-next').disabled=locked||pageStart+8>=frames.length
      el('info').textContent=exists?(selected+1)+' / '+frames.length:''
      if(exists)Object.keys(ranges).forEach(function(name){el(name).value=String(frames[selected][name])})
      renderStrip();draw()
    }
    async function painted(edit){
      var frame=source(edit),image=await options.image(frame.media);if(!image)return null
      var canvas=document.createElement('canvas');canvas.width=frame.width;canvas.height=frame.height
      var context=canvas.getContext('2d');context.drawImage(image,0,0,frame.width,frame.height)
      if(edit.eraseStrokes.length){
        var pixels=context.getImageData(0,0,frame.width,frame.height)
        pixels.data.set(applyImageAlphaStrokes(pixels,edit.eraseStrokes))
        context.putImageData(pixels,0,0)
      }
      return canvas
    }
    async function drawInto(target,edit,alpha){
      var image=await painted(edit);if(!image)return
      var c=target.getContext('2d');c.save();c.globalAlpha=edit.opacity*alpha;c.imageSmoothingEnabled=false;c.translate(target.width/2+edit.x,target.height/2+edit.y);c.rotate(edit.rotation*Math.PI/180);c.scale(edit.scale,edit.scale);c.drawImage(image,-image.width/2,-image.height/2);c.restore()
    }
    async function draw(){
      var revision=++epoch,index=selected,edit=frames[index];if(!edit)return
      var frame=source(edit),buffer=document.createElement('canvas');buffer.width=frame.width;buffer.height=frame.height
      try{if(el('onion').checked&&index>0)await drawInto(buffer,frames[index-1],0.25);await drawInto(buffer,edit,1);if(disposed||revision!==epoch)return;var canvas=el('canvas');canvas.width=buffer.width;canvas.height=buffer.height;canvas.style.width='100%';canvas.style.aspectRatio=buffer.width+'/'+buffer.height;canvas.getContext('2d').drawImage(buffer,0,0)}catch{if(!disposed&&revision===epoch)status('error')}
    }
    var stripRevision=0
    function renderStrip(){
      var revision=++stripRevision;el('strip').replaceChildren()
      var visible=frames.slice(pageStart,pageStart+8)
      visible.forEach(function(edit,offset){var index=pageStart+offset,button=document.createElement('button'),canvas=document.createElement('canvas'),span=document.createElement('span');button.type='button';button.className='edit-thumb';button.disabled=busy||applying;button.setAttribute('aria-label',w().frame+' '+(index+1));button.title=[source(edit).action,source(edit).direction].filter(Boolean).join(' · ');button.setAttribute('aria-pressed',String(index===selected));canvas.width=source(edit).width;canvas.height=source(edit).height;span.textContent=String(index+1);button.append(canvas,span);button.addEventListener('click',function(){selected=index;render()});el('strip').append(button)})
      // 串行读取当前页最多八张，避免大图同时占用桥的全部四个通道。
      ;(async function(){for(var i=0;i<visible.length;i++){if(disposed||revision!==stripRevision)return;var canvas=el('strip').children[i].querySelector('canvas');try{await drawInto(canvas,visible[i],1)}catch{if(!disposed&&revision===stripRevision)status('error')}}})()
    }
    function position(event){var rect=el('canvas').getBoundingClientRect();return{x:(event.clientX-rect.left)*el('canvas').width/rect.width,y:(event.clientY-rect.top)*el('canvas').height/rect.height}}
    function sourcePoint(point,edit){var frame=source(edit),angle=-edit.rotation*Math.PI/180,dx=point.x-frame.width/2-edit.x,dy=point.y-frame.height/2-edit.y;return{x:Math.max(0,Math.min(1,((dx*Math.cos(angle)-dy*Math.sin(angle))/edit.scale+frame.width/2)/frame.width)),y:Math.max(0,Math.min(1,((dx*Math.sin(angle)+dy*Math.cos(angle))/edit.scale+frame.height/2)/frame.height))}}
    function pointCount(){return frames.reduce(function(sum,frame){return sum+frame.eraseStrokes.reduce(function(total,stroke){return total+stroke.points.length},0)},0)}
    el('canvas').addEventListener('pointerdown',function(event){if(busy||applying||!frames[selected]||event.button>0)return;event.preventDefault();var edit=frames[selected],start=position(event);if(tool!=='move'&&(edit.eraseStrokes.length>=64||pointCount()>=20000)){status('limit');return}gesture={id:event.pointerId,before:snapshot(),start:start,x:edit.x,y:edit.y};if(tool!=='move'){gesture.stroke={points:[sourcePoint(start,edit)],radius:Number(el('brush').value)/100,restore:tool==='restore'};edit.eraseStrokes.push(gesture.stroke)}el('canvas').setPointerCapture(event.pointerId);draw()})
    el('canvas').addEventListener('pointermove',function(event){if(!gesture||event.pointerId!==gesture.id)return;var point=position(event),edit=frames[selected];if(tool==='move'){edit.x=Math.max(-4096,Math.min(4096,Math.round(gesture.x+point.x-gesture.start.x)));edit.y=Math.max(-4096,Math.min(4096,Math.round(gesture.y+point.y-gesture.start.y)))}else if(gesture.stroke.points.length<512&&pointCount()<20000){var next=sourcePoint(point,edit),last=gesture.stroke.points[gesture.stroke.points.length-1];if(Math.abs(next.x-last.x)+Math.abs(next.y-last.y)>0.001)gesture.stroke.points.push(next)}draw()})
    function endGesture(event){if(!gesture||event.pointerId!==gesture.id)return;var before=gesture.before;gesture=null;if(JSON.stringify(before.frames)!==JSON.stringify(frames)){remember(before);changed()}else render()}
    el('canvas').addEventListener('pointerup',endGesture);el('canvas').addEventListener('pointercancel',endGesture)
    el('canvas').addEventListener('keydown',function(event){if(busy||applying||!frames[selected]||!['ArrowLeft','ArrowRight','ArrowUp','ArrowDown'].includes(event.key))return;event.preventDefault();remember();var step=event.shiftKey?10:1,edit=frames[selected];if(event.key==='ArrowLeft')edit.x=Math.max(-4096,edit.x-step);if(event.key==='ArrowRight')edit.x=Math.min(4096,edit.x+step);if(event.key==='ArrowUp')edit.y=Math.max(-4096,edit.y-step);if(event.key==='ArrowDown')edit.y=Math.min(4096,edit.y+step);changed()})
    root.querySelectorAll('[data-tool]').forEach(function(button){button.addEventListener('click',function(){tool=button.dataset.tool;render()})});el('onion').addEventListener('change',draw)
    el('page-previous').addEventListener('click',function(){pageStart=Math.max(0,pageStart-8);render()});el('page-next').addEventListener('click',function(){pageStart=Math.min(Math.floor((frames.length-1)/8)*8,pageStart+8);render()})
    ;['earlier','later','duplicate','remove'].forEach(function(action){el(action).addEventListener('click',function(){if(busy||applying||!frames[selected])return;remember();if(action==='earlier'&&selected>0){var prev=frames[selected-1];frames[selected-1]=frames[selected];frames[selected]=prev;selected--}if(action==='later'&&selected<frames.length-1){var next=frames[selected+1];frames[selected+1]=frames[selected];frames[selected]=next;selected++}if(action==='duplicate'&&frames.length<512){frames.splice(selected+1,0,copy(frames[selected]));selected++}if(action==='remove'&&frames.length>1){frames.splice(selected,1);selected=Math.min(selected,frames.length-1)}pageStart=Math.floor(selected/8)*8;changed()})})
    el('undo').addEventListener('click',function(){if(busy||applying||!undo.length)return;redo.push(snapshot());restore(undo.pop());status(dirty()?'dirty':'')});el('redo').addEventListener('click',function(){if(busy||applying||!redo.length)return;undo.push(snapshot());restore(redo.pop());status(dirty()?'dirty':'')})
    el('reset').addEventListener('click',function(){if(busy||applying)return;remember();frames=original();selected=0;pageStart=0;changed()})
    el('discard').addEventListener('click',function(){if(busy||applying)return;frames=copy(job&&job.edits?job.edits.frames:saved);saved=copy(frames);undo=[];redo=[];selected=0;pageStart=0;changed()})
    el('apply').addEventListener('click',async function(){if(busy||applying||!job||!dirty())return;var selectedJob=job.id;applying=true;status('saving');render();try{var result=await options.save(selectedJob,{frames:copy(frames)});if(disposed||!result||!job||job.id!==selectedJob)return;job=result;key=jobKey(result);frames=copy(result.edits?result.edits.frames:frames);saved=copy(frames);undo=[];redo=[];status('saved')}catch{if(!disposed)status('error')}finally{applying=false;if(!disposed){render();options.onDirty()}}})
    localize()
    return {dirty:dirty,setLocale:function(value){locale=value;localize()},setBusy:function(value){if(busy===value)return;busy=value;render()},setJob:function(value){if(value&&job&&value.id===job.id&&value.revision<job.revision)return;var next=jobKey(value);if(next===key||applying)return;if(dirty())return;job=value;key=next;frames=value&&value.originalOutput?copy(value.edits?value.edits.frames:original()):[];saved=copy(frames);undo=[];redo=[];selected=0;pageStart=0;status('');render();options.onDirty()},dispose:function(){disposed=true;epoch++;stripRevision++;gesture=null}}
  }
})()
`
