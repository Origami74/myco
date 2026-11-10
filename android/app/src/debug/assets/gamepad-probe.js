// Diagnostic bootstrap only. Runs the production shell with a fake load channel;
// it neither implements nor advertises NAP-GAMEPAD.
(function () {
  'use strict';
  var report = { androidWindowFocus: null, child: null };
  var output;
  var previous;

  function sample() {
    if (typeof navigator.getGamepads !== 'function') return { status: 'missing' };
    try {
      return { status: 'readable', pads: Array.from(navigator.getGamepads(), function (pad) {
        return pad && {
          index: pad.index, id: pad.id, mapping: pad.mapping,
          timestamp: pad.timestamp, axes: Array.from(pad.axes),
          buttons: Array.from(pad.buttons, function (button) {
            return { value: button.value, pressed: button.pressed, touched: button.touched };
          })
        };
      }) };
    } catch (error) { return { status: error.name }; }
  }

  function childProbe() {
    var nestedStatus = null;
    window.addEventListener('message', function (event) {
      if (event.source === nested.contentWindow && event.data.type === 'probe.nested') {
        nestedStatus = event.data.native;
      }
    });
    var nested = document.createElement('iframe');
    nested.hidden = true;
    nested.srcdoc = '<script>var status = "missing"; try {' +
      'if (typeof navigator.getGamepads === "function") { navigator.getGamepads(); status = "readable"; }' +
      '} catch (error) { status = error.name; }' +
      'parent.postMessage({type:"probe.nested",native:status},"*");<\/script>';
    document.body.appendChild(nested);
    function send() {
      var status = typeof navigator.getGamepads === 'function' ? 'readable' : 'missing';
      try { if (status === 'readable') navigator.getGamepads(); }
      catch (error) { status = error.name; }
      parent.postMessage({ type: 'probe.result', native: status, nested: nestedStatus,
        focused: document.hasFocus(), visibility: document.visibilityState }, '*');
    }
    ['focus', 'blur', 'visibilitychange', 'gamepadconnected', 'gamepaddisconnected']
      .forEach(function (name) { window.addEventListener(name, send); });
    setInterval(send, 250);
    send();
  }

  window.addEventListener('probe-window-focus', function (event) {
    report.androidWindowFocus = event.detail;
    console.info('gamepad-probe window focus: ' + event.detail);
  });

  window.mycoNappletRuntime = {
    postMessage: function (raw) {
      var message = JSON.parse(raw);
      if (message.channel === 'napplet' && message.message.type === 'probe.result') {
        report.child = message.message;
      }
      if (message.channel !== 'shell' || message.action !== 'mounted') return;
      function mountProbe() {
        output = document.createElement('pre');
        output.style.cssText = 'position:fixed;bottom:0;left:0;right:0;max-height:75%;overflow:auto;' +
          'background:#111;color:#eee;padding:12px;margin:0;pointer-events:none;font-size:12px';
        document.body.appendChild(output);
        window.mycoNappletRuntime.onmessage({ data: JSON.stringify({
          channel: 'shell', action: 'load', sandbox: 'allow-scripts',
          artifact: '<!doctype html><meta name="viewport" content="width=device-width">' +
            '<body style="background:#172a22;color:white;font:18px sans-serif">' +
            '<p>Controller probe: tap here, then press controller buttons.</p>' +
            '<p>Switch apps, open notifications, disconnect and reconnect.</p>' +
            '<script>(' + childProbe.toString() + ')();<\/script>'
        }) });
        setInterval(function () {
          report.secureContext = window.isSecureContext;
          report.userAgent = navigator.userAgent;
          report.shell = sample();
          report.visible = document.visibilityState;
          report.documentFocus = document.hasFocus();
          report.frameFocus = document.activeElement === document.querySelector('iframe');
          report.isolated = !!report.child && report.child.native === 'SecurityError' &&
            report.child.nested === 'SecurityError';
          output.textContent = JSON.stringify(report, null, 2);
          // Log changes without dumping input snapshots every display frame.
          var summary = JSON.stringify({ isolated: report.isolated, child: report.child,
            visible: report.visible, documentFocus: report.documentFocus,
            shellStatus: report.shell.status });
          if (summary !== previous) console.info('gamepad-probe ' + summary);
          previous = summary;
        }, 100);
      }
      if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', mountProbe, { once: true });
      } else mountProbe();
    }
  };
})();
