const { execSync } = require('child_process');
const path = require('path');
const fs = require('fs');

const root = path.join(__dirname, '..');
const pkg = require(path.join(root, 'package.json'));
const VER = pkg.version;
const APP = pkg.build.productName;
const outDir = path.join(root, 'out');
const exePath = path.join(outDir, 'win-unpacked', `${APP}.exe`);
const setupPath = path.join(outDir, `${APP} Setup ${VER}.exe`);
const certPath = path.join(root, 'cert.pfx');
const signScript = path.join(__dirname, 'sign.ps1');

process.env.CSC_IDENTITY_AUTO_DISCOVERY = 'false';
const run = (cmd) => execSync(cmd, { stdio: 'inherit', cwd: root });

const sign = (filePath) => {
    if (!fs.existsSync(certPath) || !fs.existsSync(filePath)) return;
    try {
        run(`powershell -ExecutionPolicy Bypass -File "${signScript}" -ExePath "${filePath}"`);
        console.log(`  • signed: ${path.basename(filePath)}`);
    } catch (e) {
        console.log(`  • sign skipped: ${path.basename(filePath)}`);
    }
};

console.log(`\n  Building ${APP} v${VER}\n`);

if (fs.existsSync(outDir)) {
    console.log('Cleaning old build...');
    fs.rmSync(outDir, { recursive: true, force: true });
}

console.log('Step 1/3: Compile & Build...');
run('npx tsc');
run('npx electron-builder --win --publish never');
console.log('  • done');

console.log('Step 2/3: Sign exe...');
sign(exePath);

console.log('Step 3/3: Sign installer...');
sign(setupPath);

console.log(`\n  Done! ${APP} Setup ${VER}.exe ready in out/\n`);
