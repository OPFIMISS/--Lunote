// Lunote 单文件 Windows 包：安装版 (SETUP) 与便携版 (PORTABLE)
// 编译：csc /target:winexe /define:SETUP /resource:payload.zip,PAYLOAD ...
using System;
using System.Diagnostics;
using System.IO;
using System.IO.Compression;
using System.Reflection;
using System.Windows.Forms;

static class PackageLauncher
{
    [STAThread]
    static void Main()
    {
        try
        {
            Run();
        }
        catch (Exception e)
        {
            MessageBox.Show("月笺启动失败：" + e.Message, "月笺 Lunote",
                MessageBoxButtons.OK, MessageBoxIcon.Error);
        }
    }

    static string ExtractPayload(string dir)
    {
        Directory.CreateDirectory(dir);
        string zip = Path.Combine(Path.GetTempPath(),
            "lunote-payload-" + Guid.NewGuid().ToString("N") + ".zip");
        Assembly asm = Assembly.GetExecutingAssembly();
        using (Stream s = asm.GetManifestResourceStream("PAYLOAD"))
        using (FileStream fs = File.Create(zip))
        {
            s.CopyTo(fs);
        }
        using (ZipArchive archive = ZipFile.OpenRead(zip))
        {
            foreach (ZipArchiveEntry entry in archive.Entries)
            {
                if (string.IsNullOrEmpty(entry.Name)) continue;
                string outPath = Path.GetFullPath(
                    Path.Combine(dir, entry.FullName));
                if (!outPath.StartsWith(Path.GetFullPath(dir),
                        StringComparison.OrdinalIgnoreCase))
                    continue;
                Directory.CreateDirectory(Path.GetDirectoryName(outPath));
                entry.ExtractToFile(outPath, true);
            }
        }
        File.Delete(zip);
        return Path.Combine(dir, "lunote_app.exe");
    }

    static void Run()
    {
#if SETUP
        string dest = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "Lunote");
        string exe = ExtractPayload(dest);
        CreateShortcut(exe, dest, Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.DesktopDirectory),
            "Lunote.lnk"));
        CreateShortcut(exe, dest, Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.Programs),
            "Lunote.lnk"));
        Process.Start(new ProcessStartInfo(exe) { WorkingDirectory = dest });
#else
        string mutexName = "Local\\LunotePortableLaunch";
        bool created;
        using (System.Threading.Mutex mutex =
            new System.Threading.Mutex(true, mutexName, out created))
        {
            if (!created)
            {
                MessageBox.Show("月笺便携版已在运行，请等待其完成。", "月笺 Lunote",
                    MessageBoxButtons.OK, MessageBoxIcon.Information);
                return;
            }
            string dest = Path.Combine(Path.GetTempPath(), "LunotePortable");
            string exe = Path.Combine(dest, "lunote_app.exe");
            if (!File.Exists(exe)) exe = ExtractPayload(dest);
            Process p = Process.Start(
                new ProcessStartInfo(exe) { WorkingDirectory = dest });
            p.WaitForExit();
            try { Directory.Delete(dest, true); } catch { }
        }
#endif
    }

    static void CreateShortcut(string exe, string workDir, string lnkPath)
    {
        try
        {
            Type t = Type.GetTypeFromProgID("WScript.Shell");
            if (t == null) return;
            dynamic shell = Activator.CreateInstance(t);
            dynamic link = shell.CreateShortcut(lnkPath);
            link.TargetPath = exe;
            link.WorkingDirectory = workDir;
            link.IconLocation = exe + ",0";
            link.Save();
        }
        catch
        {
            // 快捷方式失败不阻塞安装
        }
    }
}
