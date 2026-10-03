/** Input, save, and repair of existing automation names through real settings.
 * node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-ids.mjs
 */
const desk = '11111111-2222-4333-8444-555555555555';
const tab = '22222222-3333-4444-8555-666666666666';
const scene = fn => '(' + fn.toString() + ')()';
export default {
  config:{desks:[{name:'Work',id:'work.dev',uid:desk,folders:[{name:'Project',tabs:[{name:'Shell',id:'prod.dev',uid:tab,command:'sh'}]}]}]},
  init:`window.check=(ok,why)=>{if(!ok)throw new Error(why)};
    window.typeIn=(input,value)=>{input.value=value;input.dispatchEvent(new Event('input'))};`,
  scenes:{
    'id-invalid':scene(async () => {
      sel={desk:0,tab:null,global:false}; render();
      const input=document.querySelector('[data-automation-id="'+desks[0].uid+'"]');
      check(input && input.getAttribute('aria-invalid')==='false','Existing id was rejected before editing');
      typeIn(input,'new.invalid');
      check(input.getAttribute('aria-invalid')==='true','Invalid id has no field error');
      check(await doSave()===false,'New invalid id was saved');
      check(document.querySelector('#detail').innerText.includes(T['settings.desk.id.hint']),'Id rules missing');
      input.scrollIntoView({block:'center'});
    }),
    'id-repaired':scene(async () => {
      check(await doSave()===true,'Unchanged legacy names could not be saved');
      desks[0].id='work'; desks[0].tabs[0].id='prod';
      check(await doSave()===true,'Legacy names could not be repaired');
      check(await doSave()===true,'A second save failed');
      const saved=await settingsApi('/api/config');
      check(saved.desks[0].id==='work' && saved.desks[0].folders[0].tabs[0].id==='prod','Repaired names were not persisted');
      sel={desk:0,tab:0,global:false}; render();
      const input=document.querySelector('[data-automation-id="'+desks[0].tabs[0].uid+'"]');
      check(input.value==='prod' && input.getAttribute('aria-invalid')==='false','Repaired field still shows an error');
      input.scrollIntoView({block:'center'});
      check(document.documentElement.scrollWidth<=innerWidth,'Id rules overflow');
    }),
  },
  langs:['ja','en'], sizes:[['wide',1280,900],['phone',390,820]],
};
