import { app, BrowserWindow, ipcMain, dialog, Menu, shell, nativeTheme } from 'electron';
import * as path from 'path';
import * as fs from 'fs';
import { autoUpdater } from 'electron-updater';

let mainWindow: BrowserWindow | null = null;
let recentFiles: string[] = [];
let RECENT_FILES_PATH = '';
let pendingFilePath: string | null = null;

app.setName('Advanced PDF Reader');

const gotSingleLock = app.requestSingleInstanceLock();
if (!gotSingleLock) {
    app.quit();
}

app.on('second-instance', (_event, argv) => {
    if (mainWindow) {
        if (mainWindow.isMinimized()) mainWindow.restore();
        mainWindow.focus();
        const pdfArg = argv.find(a => a.toLowerCase().endsWith('.pdf'));
        if (pdfArg && fs.existsSync(pdfArg)) loadPDFFile(pdfArg);
    }
});

function getPdfFromArgs(argv: string[]): string | null {
    const arg = argv.find(a => a.toLowerCase().endsWith('.pdf'));
    return (arg && fs.existsSync(arg)) ? arg : null;
}

function loadRecentFiles(): void {
    try {
        if (fs.existsSync(RECENT_FILES_PATH)) {
            const data = fs.readFileSync(RECENT_FILES_PATH, 'utf-8');
            recentFiles = JSON.parse(data);
        }
    } catch {
        recentFiles = [];
    }
}

function saveRecentFiles(): void {
    try {
        fs.writeFileSync(RECENT_FILES_PATH, JSON.stringify(recentFiles.slice(0, 20)));
    } catch (e) {
        console.error('Failed to save recent files:', e);
    }
}

function addRecentFile(filePath: string): void {
    recentFiles = recentFiles.filter(f => f !== filePath);
    recentFiles.unshift(filePath);
    recentFiles = recentFiles.slice(0, 20);
    saveRecentFiles();
    if (mainWindow) {
        mainWindow.webContents.send('recent-files-updated', recentFiles);
    }
}

function createWindow(): void {
    mainWindow = new BrowserWindow({
        width: 1400,
        height: 900,
        minWidth: 900,
        minHeight: 600,
        frame: false,
        titleBarStyle: 'hidden',
        backgroundColor: '#0a0a0f',
        icon: path.join(__dirname, '..', 'assets', 'icon.ico'),
        webPreferences: {
            preload: path.join(__dirname, 'preload.js'),
            contextIsolation: true,
            nodeIntegration: false,
            sandbox: false,
            webSecurity: true,
        },
        show: false,
    });

    mainWindow.loadFile(path.join(__dirname, '..', 'renderer', 'index.html'));

    mainWindow.once('ready-to-show', () => {
        mainWindow?.show();
        mainWindow?.webContents.send('recent-files-updated', recentFiles);
        if (pendingFilePath) {
            loadPDFFile(pendingFilePath);
            pendingFilePath = null;
        }
    });

    mainWindow.webContents.on('console-message', (event, level, message, line, sourceId) => {
        console.log(`[Renderer] ${message}`);
    });

    mainWindow.on('closed', () => {
        mainWindow = null;
    });

    mainWindow.on('maximize', () => {
        mainWindow?.webContents.send('window-state-changed', 'maximized');
    });

    mainWindow.on('unmaximize', () => {
        mainWindow?.webContents.send('window-state-changed', 'normal');
    });
}

function buildMenu(): void {
    const template: Electron.MenuItemConstructorOptions[] = [
        {
            label: 'File',
            submenu: [
                { label: 'Open PDF...', accelerator: 'CmdOrCtrl+O', click: () => openFileDialog() },
                { label: 'Save', accelerator: 'CmdOrCtrl+S', click: () => mainWindow?.webContents.send('save-pdf') },
                { label: 'Save As...', accelerator: 'CmdOrCtrl+Shift+S', click: () => mainWindow?.webContents.send('save-pdf-as') },
                { label: 'Print...', accelerator: 'CmdOrCtrl+P', click: () => mainWindow?.webContents.send('print-pdf') },
                { type: 'separator' },
                { label: 'Exit', accelerator: 'Alt+F4', click: () => app.quit() },
            ],
        },
        {
            label: 'Edit',
            submenu: [
                { label: 'Find', accelerator: 'CmdOrCtrl+F', click: () => mainWindow?.webContents.send('focus-search') },
            ],
        },
        {
            label: 'View',
            submenu: [
                { label: 'Zoom In', accelerator: 'CmdOrCtrl+Plus', click: () => mainWindow?.webContents.send('zoom', 'in') },
                { label: 'Zoom Out', accelerator: 'CmdOrCtrl+-', click: () => mainWindow?.webContents.send('zoom', 'out') },
                { label: 'Reset Zoom', accelerator: 'CmdOrCtrl+0', click: () => mainWindow?.webContents.send('zoom', 'reset') },
                { type: 'separator' },
                { label: 'Single Page', click: () => mainWindow?.webContents.send('view-mode', 'single') },
                { label: 'Continuous Scroll', click: () => mainWindow?.webContents.send('view-mode', 'continuous') },
                { type: 'separator' },
                { label: 'Toggle Sidebar', accelerator: 'CmdOrCtrl+B', click: () => mainWindow?.webContents.send('toggle-sidebar') },
                { label: 'Full Screen', accelerator: 'F11', click: () => mainWindow?.setFullScreen(!mainWindow.isFullScreen()) },
                { type: 'separator' },
                { role: 'toggleDevTools' },
            ],
        },
        {
            label: 'Tools',
            submenu: [
                { label: 'Rotate Left', click: () => mainWindow?.webContents.send('rotate', -90) },
                { label: 'Rotate Right', click: () => mainWindow?.webContents.send('rotate', 90) },
                { type: 'separator' },
                { label: 'Document Info', accelerator: 'CmdOrCtrl+I', click: () => mainWindow?.webContents.send('show-info') },
            ],
        },
        {
            label: 'Help',
            submenu: [
                { label: 'Keyboard Shortcuts', click: () => mainWindow?.webContents.send('show-shortcuts') },
                { label: 'Check for Updates', click: () => autoUpdater.checkForUpdatesAndNotify() },
                { type: 'separator' },
                { label: 'About Advanced PDF Reader', click: () => mainWindow?.webContents.send('show-about') },
            ],
        },
    ];

    const menu = Menu.buildFromTemplate(template);
    Menu.setApplicationMenu(menu);
}

async function openFileDialog(): Promise<void> {
    if (!mainWindow) return;

    const result = await dialog.showOpenDialog(mainWindow, {
        properties: ['openFile'],
        filters: [{ name: 'PDF Files', extensions: ['pdf'] }],
    });

    if (!result.canceled && result.filePaths.length > 0) {
        const filePath = result.filePaths[0];
        loadPDFFile(filePath);
    }
}

function loadPDFFile(filePath: string): void {
    if (!mainWindow) return;
    try {
        const fileBuffer = fs.readFileSync(filePath);
        const uint8Array = new Uint8Array(fileBuffer);
        addRecentFile(filePath);
        mainWindow.webContents.send('pdf-loaded', {
            data: Array.from(uint8Array),
            fileName: path.basename(filePath),
            filePath: filePath,
            fileSize: fileBuffer.length,
        });
    } catch (err) {
        dialog.showErrorBox('Error', `Failed to open PDF file: ${err}`);
    }
}

// IPC Handlers
ipcMain.handle('open-file-dialog', async () => {
    await openFileDialog();
});

ipcMain.handle('get-recent-files', () => {
    return recentFiles;
});

ipcMain.handle('open-recent-file', (_event, filePath: string) => {
    if (fs.existsSync(filePath)) {
        loadPDFFile(filePath);
    } else {
        recentFiles = recentFiles.filter(f => f !== filePath);
        saveRecentFiles();
        dialog.showErrorBox('File Not Found', `The file "${filePath}" no longer exists.`);
    }
});

ipcMain.handle('window-minimize', () => {
    mainWindow?.minimize();
});

ipcMain.handle('window-maximize', () => {
    if (mainWindow?.isMaximized()) {
        mainWindow.unmaximize();
    } else {
        mainWindow?.maximize();
    }
});

ipcMain.handle('window-close', () => {
    mainWindow?.close();
});

ipcMain.handle('is-maximized', () => {
    return mainWindow?.isMaximized() ?? false;
});

ipcMain.handle('toggle-fullscreen', () => {
    if (mainWindow) {
        mainWindow.setFullScreen(!mainWindow.isFullScreen());
    }
});

ipcMain.handle('get-theme', () => {
    return nativeTheme.shouldUseDarkColors ? 'dark' : 'light';
});

ipcMain.handle('show-file-in-folder', (_event, filePath: string) => {
    shell.showItemInFolder(filePath);
});

ipcMain.handle('save-pdf', async (_event, data: number[], filePath: string) => {
    if (!mainWindow || !filePath) return { ok: false };
    try {
        const buf = Buffer.from(data);
        fs.writeFileSync(filePath, buf);
        return { ok: true, filePath };
    } catch (e) {
        return { ok: false, error: String(e) };
    }
});

ipcMain.handle('save-pdf-as', async (_event, data: number[]) => {
    if (!mainWindow) return { ok: false };
    const result = await dialog.showSaveDialog(mainWindow, {
        filters: [
            { name: 'PDF Document', extensions: ['pdf'] },
            { name: 'PNG Image', extensions: ['png'] },
            { name: 'JPEG Image', extensions: ['jpg', 'jpeg'] },
        ],
    });
    if (result.canceled || !result.filePath) return { ok: false, canceled: true };
    try {
        fs.writeFileSync(result.filePath, Buffer.from(data));
        const ext = path.extname(result.filePath).toLowerCase();
        if (ext === '.pdf') addRecentFile(result.filePath);
        return { ok: true, filePath: result.filePath, ext };
    } catch (e) {
        return { ok: false, error: String(e) };
    }
});

ipcMain.handle('print-pdf', async (_event, data?: number[]) => {
    if (!mainWindow || !data || data.length === 0) return;
    const tempPath = path.join(app.getPath('temp'), `apdf-print-${Date.now()}.pdf`);
    fs.writeFileSync(tempPath, Buffer.from(data));
    await shell.openPath(tempPath);
});

function setupAutoUpdater(): void {
    autoUpdater.autoDownload = true;
    autoUpdater.autoInstallOnAppQuit = true;

    autoUpdater.on('update-available', (info) => {
        mainWindow?.webContents.send('update-status', `Update v${info.version} available, downloading...`);
    });

    autoUpdater.on('update-downloaded', (info) => {
        dialog.showMessageBox(mainWindow!, {
            type: 'info',
            title: 'Update Ready',
            message: `Version ${info.version} has been downloaded. Restart to apply the update.`,
            buttons: ['Restart Now', 'Later'],
        }).then((r) => {
            if (r.response === 0) autoUpdater.quitAndInstall();
        });
    });

    autoUpdater.on('error', (err) => {
        console.log('Auto-updater error:', err.message);
    });

    autoUpdater.checkForUpdatesAndNotify();
}

app.whenReady().then(() => {
    RECENT_FILES_PATH = path.join(app.getPath('userData'), 'recent-files.json');
    loadRecentFiles();
    buildMenu();
    pendingFilePath = getPdfFromArgs(process.argv);
    createWindow();
    setupAutoUpdater();
});

app.on('window-all-closed', () => {
    if (process.platform !== 'darwin') {
        app.quit();
    }
});

app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) {
        createWindow();
    }
});

app.on('open-file', (event, filePath) => {
    event.preventDefault();
    if (mainWindow) loadPDFFile(filePath);
    else pendingFilePath = filePath;
});
