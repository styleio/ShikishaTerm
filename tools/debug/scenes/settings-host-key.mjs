/** Standalone settings asks before trusting SSH, without a board or native dialog.
 * node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-host-key.mjs
 * Network fixtures here; ssh::tests covers these endpoints against a real SSH server.
 */
const init = `
  const realFetch = window.fetch.bind(window);
  window.keyQuestions = [];
  window.keyAnswers = [];
  window.fetch = async (url, options) => {
    const u = new URL(typeof url === 'string' ? url : url.url, location.href);
    const json = value => new Response(JSON.stringify(value), {headers:{'Content-Type':'application/json'}});
    if (u.pathname === '/api/server/keys') return json({keys:window.keyQuestions});
    if (u.pathname === '/api/server/key') {
      const answer = JSON.parse(options.body);
      window.keyAnswers.push(answer);
      window.keyQuestions = [];
      return json({ok:true, trusted:answer.trust});
    }
    return realFetch(url, options);
  };
  window.until = async (test, why) => {
    for (let i=0;i<100;i++) { if (test()) return; await new Promise(r=>setTimeout(r,50)); }
    throw new Error(why);
  };
  window.check = (ok, why) => { if (!ok) throw new Error(why); };
`;
const question = changed => ({
  machine:'build.example.com:22', before:changed ? 'SHA256:mjls4MHrZmcuZqo4Enuc8KUKhm02OCHDxsrRXPuCl20' : '',
  now:'SHA256:WcVVHHZRwUrIoImRBg17hTFohfvS5paU5WpYS8UKWYQ',
});
const scene = changed => `(${async function(q) {
  for (const trust of [false, true]) {
    window.keyQuestions = [q];
    const checking = checkServerKeys();
    await until(()=>document.querySelector('.confirm-box'), 'No SSH key question');
    const box = document.querySelector('.confirm-box');
    check(box.innerText.includes(q.machine) && box.innerText.includes(q.now), 'The server or fingerprint is missing');
    check(window.keyAnswers.length === (trust ? 1 : 0), 'A key was answered before a person chose');
    check(document.activeElement.textContent === T['common.cancel'], 'Trust is selected by default');
    box.querySelector(trust ? '.danger' : '.mfoot .quiet').click();
    await checking;
    const sent = window.keyAnswers.at(-1);
    check(sent.machine === q.machine && sent.fingerprint === q.now && sent.trust === trust, 'The displayed key was not answered exactly');
  }
  result('');
  window.keyQuestions = [q];
  void checkServerKeys();
  await until(()=>document.querySelector('.confirm-box'), 'Final question missing');
  check(document.documentElement.scrollWidth <= innerWidth, 'Question overflows horizontally');
}})(${JSON.stringify(question(changed))})`;
export default {
  init,
  config:{desks:[{name:'Work',id:'work',folders:[{name:'Build',tabs:[{name:'shell',command:'sh'}]}]}]},
  scenes:{'ssh-first-key':scene(false), 'ssh-changed-key':scene(true)},
  langs:['ja','en'], sizes:[['wide',1280,900],['phone',390,820]],
};
