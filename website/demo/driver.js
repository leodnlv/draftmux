// Plays the scene for the landing page, which embeds it as index.html?web.
// The page sends { demo: 'play' | 'pause' | 'seek', t } and gets { demo:
// 'state', t, playing, start, duration, ready, chapters } back on every frame
// and change. chapters lists the scene's segments as { label, start, end }. Without ?web (in the scenecast player or a render) this does
// nothing.
(() => {
  if (!new URLSearchParams(location.search).has('web')) return;
  const START = 0, END = window.DURATION;
  let t = START, playing = false, last = 0, raf = 0, ready = false;
  const chapters = (window.SEGMENTS || []).map(({ label, start, end }) => ({ label, start, end }));
  const send = () => parent.postMessage({ demo: 'state', t, playing, start: START, duration: END, ready, chapters }, '*');
  const clamp = (x) => Math.min(END, Math.max(START, x));
  // The scene is authored at 60 fps. A faster display skips the frames that
  // come sooner than that, rather than drawing the scene twice as often.
  // Each frame is due a 60th of a second after the last one; display frames
  // that land a little early still count, so a 60 Hz display draws them all.
  const FRAME = 1000 / 60, SLACK = 2.5;
  let due = 0;
  function frame(now) {
    if (now < due - SLACK) { raf = requestAnimationFrame(frame); return; }
    due = Math.max(due + FRAME, now + FRAME - SLACK);
    // A hidden tab stops frames; don't jump ahead when it comes back.
    t = clamp(t + Math.min(now - last, 100) / 1000);
    last = now;
    window.seek(t);
    if (t >= END) playing = false;
    send();
    if (playing) raf = requestAnimationFrame(frame);
  }
  function play() {
    if (!ready || playing) return;
    if (t >= END) { t = START; window.seek(t); }
    playing = true;
    last = performance.now();
    raf = requestAnimationFrame(frame);
    send();
  }
  function pause() {
    playing = false;
    cancelAnimationFrame(raf);
    send();
  }
  addEventListener('message', (e) => {
    if (e.source !== parent || !e.data) return;
    const { demo } = e.data;
    if (demo === 'play') play();
    else if (demo === 'pause') pause();
    else if (demo === 'seek' && ready) { t = clamp(e.data.t); window.seek(t); send(); }
  });
  Promise.resolve(window.ready).then(() => {
    ready = true;
    window.seek(t);
    send();
  });
})();
