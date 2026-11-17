// Makes sure Kumospace always runs with the CDP debugging port.
//
//   node launcher.js launch   — start Kumospace with the port (or restart it if it runs without it)
//   startWatchdog()           — used by kumowatch: if Kumospace was just opened some other way
//                               (Raycast ⌥6, Dock, Login Items), relaunch it with the port.
const { execFileSync, execFile } = require('child_process');

const APP_BIN = '/Applications/Kumospace.app/Contents/MacOS/Kumospace';
const PORT = Number(process.env.KUMO_CDP_PORT || 9222);
const FRESH_SECONDS = 120; // only auto-restart instances younger than this (never kill an active session)

const sh = (cmd, args) => { try { return execFileSync(cmd, args, { encoding: 'utf8' }).trim(); } catch { return ''; } };

// Main Kumospace process: { pid, args, ageSec } or null.
function findKumospace() {
  const out = sh('ps', ['-axo', 'pid=,etime=,args=']);
  for (const line of out.split('\n')) {
    const m = line.trim().match(/^(\d+)\s+(\S+)\s+(.*)$/);
    if (m && (m[3] === APP_BIN || m[3].startsWith(APP_BIN + ' '))) return { pid: Number(m[1]), ageSec: parseEtime(m[2]), args: m[3] };
  }
  return null;
}

function parseEtime(s) { // [[dd-]hh:]mm:ss
  const [d, rest] = s.includes('-') ? s.split('-') : [0, s];
  const parts = rest.split(':').map(Number);
  while (parts.length < 3) parts.unshift(0);
  return Number(d) * 86400 + parts[0] * 3600 + parts[1] * 60 + parts[2];
}

const hasPort = (p) => p.args.includes(`--remote-debugging-port=${PORT}`);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function quitKumospace() {
  sh('osascript', ['-e', 'quit app "Kumospace"']);
  for (let i = 0; i < 40 && findKumospace(); i++) await sleep(250);
  const p = findKumospace();
  if (p) { try { process.kill(p.pid, 'SIGTERM'); } catch {} await sleep(1500); }
}

function openWithPort() {
  execFile('open', ['-a', 'Kumospace', '--args', `--remote-debugging-port=${PORT}`]);
}

// Start, focus, or restart-with-port. Safe to call any number of times.
async function launch() {
  const p = findKumospace();
  if (p && hasPort(p)) { sh('open', ['-a', 'Kumospace']); return 'focused'; }
  if (p) await quitKumospace();
  openWithPort();
  return p ? 'restarted with port' : 'started';
}

function startWatchdog({ log = console.log, intervalMs = 3000 } = {}) {
  let warnedPid = null;
  let busyUntil = 0;
  setInterval(async () => {
    if (Date.now() < busyUntil) return;
    const p = findKumospace();
    if (!p || hasPort(p)) return;
    if (p.ageSec <= FRESH_SECONDS) {
      busyUntil = Date.now() + 20000;
      log(`watchdog: Kumospace (pid ${p.pid}) started without debug port, relaunching`);
      await quitKumospace();
      openWithPort();
    } else if (warnedPid !== p.pid) {
      warnedPid = p.pid;
      log(`watchdog: Kumospace (pid ${p.pid}) runs without debug port for ${p.ageSec}s; not touching an active session. Run "node launcher.js launch" to fix.`);
    }
  }, intervalMs).unref?.();
}

module.exports = { launch, startWatchdog, findKumospace };

if (require.main === module && process.argv[2] === 'launch') launch().then((r) => console.log(r));
