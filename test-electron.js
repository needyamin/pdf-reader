const electron = require('electron');
console.log('electron type:', typeof electron);
console.log('electron keys:', Object.keys(electron));
console.log('app:', typeof electron.app);
console.log('ipcMain:', typeof electron.ipcMain);
console.log('BrowserWindow:', typeof electron.BrowserWindow);

if (electron.app) {
    electron.app.whenReady().then(() => {
        console.log('App is ready!');
        electron.app.quit();
    });
} else {
    console.log('app is not available - electron module may not be loading correctly');
    process.exit(1);
}
