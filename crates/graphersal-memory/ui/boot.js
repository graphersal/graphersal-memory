// graphersal-memory ui: the first script of the page (a classic script, before the playground's
// modules). It takes the token out of the address (`#token=<hex>`, put there by the launch file
// `ui-<pid>.html` or the printed address) into this tab's sessionStorage and clears it from the
// address, hands `Authorization: Bearer <token>` to the playground's engine through its host hook
// (`window.GRAPHERSAL_ENGINE = {headers}`, read once when the engine starts; every request of the
// engine to `/api/` carries it; ui/api-ui.js adds it to the `/api/ui/*` calls), and sends the
// heartbeat that keeps `graphersal-memory ui` running (every 60 s; the server stops 15 minutes
// after the last one). A tab whose server is gone says how to start it again. Never a cookie,
// never the token in a URL that leaves the fragment, never `window.fetch` replaced.
(function () {
  'use strict';
  var KEY = 'graphersal-memory.ui.token';
  var HEARTBEAT_MS = 60 * 1000;
  var token = '';
  try {
    var hash = window.location.hash || '';
    var match = /^#token=([0-9a-f]{64})(?:&tab=([a-z][a-z0-9-]*))?/.exec(hash);
    if (match) {
      token = match[1];
      try { window.sessionStorage.setItem(KEY, token); } catch (e) { /* storage blocked: this load only */ }
      var tab = match[2];
      window.history.replaceState(null, '', window.location.pathname + (tab ? '#/play?tab=' + tab : ''));
    } else {
      token = window.sessionStorage.getItem(KEY) || '';
    }
  } catch (e) {
    token = '';
  }

  if (token) {
    window.GRAPHERSAL_ENGINE = { headers: { Authorization: 'Bearer ' + token } };
  }

  function banner(text) {
    var show = function () {
      var el = document.getElementById('memory-ui-banner');
      if (!el) {
        el = document.createElement('div');
        el.id = 'memory-ui-banner';
        el.setAttribute('role', 'alert');
        document.body.insertBefore(el, document.body.firstChild);
      }
      el.textContent = text;
    };
    if (document.body) show(); else document.addEventListener('DOMContentLoaded', show);
  }
  function clearBanner() {
    var el = document.getElementById('memory-ui-banner');
    if (el) el.remove();
  }

  if (!token) {
    banner('This tab has no access token. Open the address that "graphersal-memory ui" printed in your terminal, or start it again: graphersal-memory ui');
    return;
  }

  function heartbeat() {
    window.fetch('/api/ui/heartbeat', {
      method: 'POST',
      cache: 'no-store',
      headers: { Authorization: 'Bearer ' + token },
    }).then(function (r) {
      if (r.status === 401) {
        banner('This tab\'s access token is no longer valid (the UI was started again). Open the new address that "graphersal-memory ui" printed.');
      } else if (r.ok) {
        clearBanner();
      }
    }, function () {
      banner('The memory UI is not running any more. Start it again in a terminal: graphersal-memory ui');
    });
  }
  heartbeat();
  window.setInterval(heartbeat, HEARTBEAT_MS);
  document.addEventListener('visibilitychange', function () {
    if (document.visibilityState === 'visible') heartbeat();
  });
})();
