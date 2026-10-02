/** All consumers of the shared fields and validation, through real DOM events.
 * node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-shared.mjs --remote-http
 * Credential and GitHub lookups use the same isolated fixtures as the PAT check.
 */
import pat from './settings-pat.mjs';

const scene = fn => '(' + fn.toString() + ')()';
const init = pat.init + `
  window.verifyHeld = async open => {
    open();
    await until(() => lastModal()?.querySelector('.mfoot .primary'), 'Form did not open');
    const modal = lastModal(), save = modal.querySelector('.mfoot .primary');
    save.click();
    await until(() => !modal.querySelector('.why').hidden, 'Held save did not explain');
    const bad = modal.querySelector('[aria-invalid=true]');
    check(bad && document.activeElement === bad, 'Held save did not focus the field');
    const note = bad.parentElement.querySelector('.site-warn');
    bad.dispatchEvent(new Event('input'));
    check(note === bad.parentElement.querySelector('.site-warn'), 'Warning was rebuilt unchanged');
    typeIn(bad, 'example');
    check(bad.getAttribute('aria-invalid') === 'false', 'Corrected field stayed invalid');
    const label = modal.querySelector('label[for="' + bad.id + '"]');
    if (label) { label.click(); check(document.activeElement === bad, 'Label does not focus its input'); }
    check(document.documentElement.scrollWidth <= innerWidth, 'Form overflowed the screen');
    return modal;
  };
`;

export default {
  ...pat, init,
  scenes: {
    'shared-provider': scene(async () => {
      const modal = await verifyHeld(() => providerDialog(null, () => {}));
      const url = modal.querySelector('input[placeholder*="https://"]');
      check(url, 'Provider URL missing');
      typeIn(url, 'https://example.com:99999');
      check(url.getAttribute('aria-invalid') === 'true', 'Port above 65535 accepted');
      typeIn(url, 'https://example.com:443');
      check(url.getAttribute('aria-invalid') === 'false', 'Valid port rejected');
    }),
    'shared-action': scene(async () => {
      const modal = await verifyHeld(() => actionDialog(null, () => {}));
      typeIn(modal.querySelector('textarea'), 'echo hello');
      check(!modal.querySelector('.primary').classList.contains('held'), 'Complete action blocked');
      check(modal.querySelector('.why').hidden, 'Old error remained');
    }),
    'shared-action-folder': scene(async () => {
      const modal = await verifyHeld(() => actionDialog(null, () => {}, 'folder'));
      check(!modal.querySelector('.primary').classList.contains('held'), 'Complete folder blocked');
    }),
    'shared-host': scene(async () => {
      hostDialog(null, () => {}, 'ssh');
      const modal = lastModal(), name = modal.querySelector('input');
      typeIn(name, '');
      // It is already open; use the same user gestures as the other forms.
      await verifyHeld(() => {});
      const address = modal.querySelector('input[placeholder="' + T['settings.hosts.at.ph'] + '"]');
      typeIn(address, 'ssh://me@example.com:22');
      check(!modal.querySelector('.mfoot .primary').classList.contains('held'), 'Complete SSH host blocked');
    }),
    'shared-notify': scene(async () => {
      desks[0].notify = {};
      const modal = await verifyHeld(() => chatDialog(desks[0], null, () => {}));
      typeIn(modal.querySelector('input[type=password]'), 'https://example.com/fixture');
      check(!modal.querySelector('.mfoot .primary').classList.contains('held'), 'Complete destination blocked');
    }),
    'shared-pat': scene(async () => {
      const modal = await verifyHeld(() => gitAccountDialog(null, () => {}));
      typeIn(modal.querySelector('input[type=password]'), 'github_pat_fixture');
      check(!modal.querySelector('.mfoot .primary').classList.contains('held'), 'Complete PAT blocked');
    }),
    'shared-secret': scene(async () => {
      secretDialog(desks[0], null);
      const modal = lastModal(), url = modal.querySelector('.site-row input');
      typeIn(url, 'https://example.com:99999');
      const save = modal.querySelector('.mfoot .primary');
      save.click();
      check(document.activeElement === url && !modal.querySelector('.why').hidden, 'Secret error is unreachable');
      check(url.getAttribute('aria-invalid') === 'true', 'Secret accepted invalid port');
      typeIn(url, 'https://example.com:443');
      check(!save.classList.contains('held') && modal.querySelector('.why').hidden, 'Fixed secret stayed blocked');
      typeIn(url, 'http://example.com');
      check(save.classList.contains('held'), 'Plain HTTP did not require its existing agreement');
    }),
  },
};
