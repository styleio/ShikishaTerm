/**
 * PAT registration and Git/gh sign-ins, through the real settings page.
 * node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-pat.mjs
 * GitHub and credential stores are fixtures; PAT writes use temporary storage.
 * Checks two PATs of one user, a rename, a replacement and persistence.
 */
import path from 'node:path';

const REPO = path.resolve(import.meta.dirname, '..', '..', '..').replaceAll('\\', '/');
const accounts = [
  { name: 'vm', label: 'SHIKISHA_VM_TOKEN', login: 'octocat' },
  { name: 'contract', label: 'Contract', login: 'octocat' },
  { name: 'classic', label: 'Legacy tools', login: 'octocat' },
];

const init = `
  const realFetch = window.fetch.bind(window);
  window.credentialWrites = [];
  window.fetch = async (url, options) => {
    const u = new URL(typeof url === 'string' ? url : url.url, location.href);
    const json = value => Promise.resolve(new Response(JSON.stringify(value), {headers:{'Content-Type':'application/json'}}));
    if (u.pathname === '/api/pc-accounts') return json({accounts:['octocat']});
    if (u.pathname === '/api/gh-accounts') return json({installed:true, accounts:[{host:'github.com',login:'octocat',active:true}]});
    if (u.pathname.startsWith('/api/pc-accounts/') || u.pathname.startsWith('/api/gh-accounts/')) {
      window.credentialWrites.push({url:u.pathname, body:JSON.parse(options.body)});
      return json({ok:true,login:'octocat'});
    }
    if (u.pathname === '/api/github') return json({signed_in:true,source:'token',login:'octocat',expires_days:30,
      kind:u.searchParams.get('account') === 'classic' ? 'classic' : 'fine'});
    return realFetch(url, options);
  };
  window.until = async (test, why) => {
    for (let i=0;i<100;i++) { if (test()) return; await new Promise(r=>setTimeout(r,50)); }
    throw new Error(why);
  };
  window.check = (ok, why) => { if (!ok) throw new Error(why); };
  window.typeIn = (input, value) => { input.value=value; input.dispatchEvent(new Event('input')); };
  window.lastModal = () => Array.from(document.querySelectorAll('.modal')).at(-1);
  window.openAccounts = async () => {
    current.git_accounts = ${JSON.stringify(accounts)};
    sel = {desk:0, tab:null, global:true}; goSection('gitaccounts','center');
    await until(()=>document.querySelector('#app-gitaccounts .secretrow .chip')?.textContent === T['settings.gitacct.kind.fine'], 'PAT kinds missing');
    window.scrollTo(0,0);
  };
  window.openPat = async () => {
    await openAccounts();
    document.querySelector('#app-gitaccounts .card button').click();
    await until(()=>lastModal()?.querySelector('input[type=password]'), 'PAT dialog missing');
    await until(()=>lastModal().querySelector('.warn')?.textContent, 'Secret storage mode missing');
  };
`;

const scene = fn => '(' + fn.toString() + ')()';

export default {
  init,
  config: {
    git_accounts: accounts,
    desks: [{
      name: 'Work', id: 'work',
      projects: [{ name: 'ShikishaTerm', at: REPO, git_account: 'vm' }],
      folders: [{ name: 'ShikishaTerm', cwd: REPO, project: 'ShikishaTerm',
        tabs: [{ name: 'shell', id: 'shell', command: 'cmd.exe' }] }],
    }],
  },
  scenes: {
    'settings-identities': scene(async () => {
      const desk = desks[0];
      sel = {desk:0, tab:null, global:false};
      const tab = desk.tabs[addTabTo(desk, 0)];
      const project = ensureProject(desk, {name:'New project', folders:[]});
      const before = desk.tabs.length;
      addTemplate('review');
      const made = {name:'New desk', projects:[{name:'Copied project', uid:'old-project'}], tabs:[]};
      await landOnWs(made);
      const ids = [tab.uid, project.uid, made.uid, made.projects[0].uid,
        ...desk.tabs.slice(before).map(t=>t.uid)];
      check(ids.length === 6 && new Set(ids).size === ids.length, 'New entries do not have separate identities');
      check(ids.every(id=>/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(id)), 'Identity format changed');
    }),
    'pat-accounts': scene(async () => {
      await openAccounts();
      check(document.querySelectorAll('#app-gitaccounts .secretrow').length === 3, 'PATs of one user collapsed');
      check(document.querySelector('#app-gitaccounts').innerText.includes(T['settings.gitacct.kind.classic']), 'Classic mislabeled');
      check(!document.querySelector('#app-gitaccounts').innerText.includes(T['settings.gitacct.where']), 'Long explanation remains');
      check(document.documentElement.scrollWidth <= innerWidth, 'Page overflows horizontally');
    }),
    'pat-dialog': scene(async () => {
      await openPat();
      const modal = lastModal();
      typeIn(modal.querySelector('input[type=text]'), 'My VM token');
      typeIn(modal.querySelector('input[type=password]'), 'github_pat_screenshot_fixture');
      check(!modal.querySelector('select'), 'PAT dialog still asks for an authentication method');
      check(modal.querySelector('.foldbody').hidden, 'Advanced fields are open by default');
      check(document.documentElement.scrollWidth <= innerWidth, 'Dialog overflows horizontally');
    }),
    'pat-help': scene(async () => {
      await openPat();
      const modal = lastModal();
      Array.from(modal.querySelectorAll('.foldhead')).find(b=>b.textContent.includes(T['settings.gitacct.token_help'])).click();
      check(modal.querySelector('a[href="https://github.com/settings/personal-access-tokens/new"]'), 'Creation link missing');
      for (const k of ['repos', 'perm_contents', 'perm_pulls', 'perm_issues', 'perm_actions', 'perm_workflows', 'classic']) {
        check(modal.innerText.includes(T['settings.gitacct.token_'+k]), 'Missing PAT guidance: '+k);
      }
      check(document.documentElement.scrollWidth <= innerWidth, 'PAT help overflows horizontally');
    }),
    'pat-pc-update': scene(async () => {
      await openAccounts();
      Array.from(document.querySelectorAll('#detail button')).find(b=>b.textContent === T['settings.gitacct.pc_add']).click();
      await until(()=>lastModal()?.querySelector('input[type=password]'), 'Git dialog missing');
      check(lastModal().innerText.includes(T['settings.gitacct.pc_add_hint']), 'Replacement is not explained');
      check(lastModal().querySelector('.primary').textContent === T['settings.gitacct.pc_save'], 'Destination missing from save button');
    }),
    'pat-other-methods': scene(async () => {
      await openAccounts();
      document.querySelector('#app-gitaccounts .foldhead').click();
      const buttons = Array.from(document.querySelectorAll('#app-gitaccounts button'));
      check(buttons.some(b=>b.textContent === T['settings.gitacct.add_ssh']), 'SSH cannot be added');
      check(buttons.some(b=>b.textContent === T['settings.gitacct.add_gh']), 'GitHub CLI cannot be selected');
    }),
    'pat-flow': scene(async () => {
      await openAccounts();
      const make = async (name, token) => {
        document.querySelector('#app-gitaccounts .card button').click();
        await until(()=>lastModal()?.querySelector('.warn')?.textContent, 'Secret storage mode missing');
        const box=lastModal();
        typeIn(box.querySelector('input[type=text]'), name);
        typeIn(box.querySelector('input[type=password]'), token);
        typeIn(box.querySelector('input[placeholder="'+T['settings.gitacct.login_ph']+'"]'), 'octocat');
        box.querySelector('.primary').click();
        await until(()=>!box.isConnected, 'PAT was not saved');
        return appGitAccounts().find(a=>a.label===name);
      };
      const a=await make('VM test 日本語', 'github_pat_vm_fixture');
      const b=await make('Contract test', 'github_pat_contract_fixture');
      check(a && b && a.name!==b.name && a.login===b.login, 'Two PATs for one login were not kept separately');
      const before=a.name;
      desks[0].projects[0].git_account=before;
      gitAccountDialog(before, ()=>{});
      await until(()=>lastModal()?.querySelector('input[type=password]')?.placeholder===T['settings.gitacct.token_set_ph'], 'Saved PAT not found');
      const edit=lastModal();
      typeIn(edit.querySelector('input[type=text]'),'VM renamed 日本語');
      edit.querySelector('.primary').click();
      await until(()=>!edit.isConnected,'Rename failed without re-entering PAT');
      check(a.name===before && desks[0].projects[0].git_account===before, 'Rename changed project binding');
      gitAccountDialog(b.name, ()=>{});
      await until(()=>lastModal()?.querySelector('input[type=password]')?.placeholder===T['settings.gitacct.token_set_ph'], 'Second PAT missing');
      const replace=lastModal();
      typeIn(replace.querySelector('input[type=password]'),'github_pat_replacement_fixture');
      replace.querySelector('.primary').click();
      await until(()=>!replace.isConnected,'Replacement failed');
      const secrets=await fetchSecrets();
      check(secrets.secrets.some(s=>s.key===gitTokenKey(a.name)) && secrets.secrets.some(s=>s.key===gitTokenKey(b.name)), 'Replacing one PAT lost another');
      await save();
      const persisted=await settingsApi('/api/config');
      check(persisted.git_accounts.some(x=>x.name===before && x.label==='VM renamed 日本語'), 'Rename not persisted');
      check(persisted.desks[0].projects[0].git_account===before,'Project selection not persisted');
      check(!JSON.stringify(persisted).includes('github_pat_'),'Token leaked into settings');
      check(!window.credentialWrites.length,'App PAT touched Git/gh sign-ins');
      sel={desk:0,proj:'p:ShikishaTerm',grp:null,tab:null,global:false,psection:'basic'}; render();
      await until(()=>document.querySelector('#project-gitacct select'),'Project picker missing');
      const picker=document.querySelector('#project-gitacct select');
      check(picker.value===before,'Project picker lost the renamed PAT');
      check(Array.from(picker.options).some(o=>o.value===b.name),'Second PAT cannot be selected');
      check(picker.querySelectorAll('optgroup').length>=3,'Sources are not separated in picker');
      document.querySelector('#project-gitacct').scrollIntoView({block:'center'});
    }),
  },
  langs: ['ja', 'en'],
  sizes: [['wide', 1280, 900], ['phone', 390, 820]],
};
