using System.IO;
using System.Windows;
namespace XeniaWorker;
public partial class App : Application
{
    FileStream? desktopLock;
    protected override void OnStartup(StartupEventArgs e)
    {
        try {
            var directory = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "XeniaWorker");
            Directory.CreateDirectory(directory);
            desktopLock = new FileStream(Path.Combine(directory, "desktop.lock"), FileMode.OpenOrCreate, FileAccess.ReadWrite, FileShare.None);
        } catch (Exception error) when (error is IOException or UnauthorizedAccessException) {
            MessageBox.Show("Xenia Worker or its installer is already open, or its settings directory is unavailable. Close the other app or installer and try again.", "Xenia Worker", MessageBoxButton.OK, MessageBoxImage.Information);
            Shutdown(1);
            return;
        }
        base.OnStartup(e);
    }
    protected override void OnExit(ExitEventArgs e)
    {
        desktopLock?.Dispose();
        base.OnExit(e);
    }
}
