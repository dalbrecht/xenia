using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Text.Json;
using System.Windows;

namespace XeniaWorker;
public partial class MainWindow : Window
{
    sealed record Config(string Node, string Name, string Root, int Capacity, int Port, string SshHost, int RemotePort, bool CodexInteractive = false, bool ClaudeInteractive = false);
    readonly string directory = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "XeniaWorker");
    Process? worker, tunnel;
    string? stopFile;
    bool stopping, tunnelRetryScheduled;
    string[]? tunnelArguments;
    CancellationTokenSource? tunnelLifetime;
    int tunnelFailures, tunnelGeneration;
    DateTime tunnelStartedAt;
    void LaunchTunnel() {
        if(tunnelArguments is null) throw new InvalidOperationException("SSH configuration is missing.");
        var ssh=new ProcessStartInfo("ssh.exe") { UseShellExecute=false,CreateNoWindow=true,RedirectStandardError=true };
        foreach(var arg in tunnelArguments) ssh.ArgumentList.Add(arg);
        var process=new Process {StartInfo=ssh,EnableRaisingEvents=true};
        process.ErrorDataReceived+=(_,e)=>Log(e.Data);
        process.Exited+=(_,_)=>Dispatcher.BeginInvoke(new Action(()=>TunnelExited(process)));
        tunnel=process;
        try { if(!process.Start())throw new InvalidOperationException("Could not start SSH."); process.BeginErrorReadLine();tunnelStartedAt=DateTime.UtcNow; }
        catch { tunnel=null;process.Dispose();throw; }
    }
    void TunnelExited(Process process) {
        if(!ReferenceEquals(tunnel,process))return;
        tunnel=null;process.Dispose();
        if(worker is null)return;
        if(DateTime.UtcNow-tunnelStartedAt>=TimeSpan.FromSeconds(30))tunnelFailures=0;
        Status.Text=stopping?"Stopping; reconnecting SSH to deliver reports…":"SSH tunnel disconnected; reconnecting…";
        _=RetryTunnelAsync();
    }
    async Task RetryTunnelAsync() {
        if(tunnelRetryScheduled || worker is null || tunnelArguments is null || tunnelLifetime is null)return;
        tunnelRetryScheduled=true;
        var delay=TimeSpan.FromSeconds(Math.Min(Math.Pow(2,Math.Min(tunnelFailures++,5)),30));
        var token=tunnelLifetime.Token;
        var generation=tunnelGeneration;
        Exception? failure=null;
        try {
            await Task.Delay(delay,token);
            if(worker is null || token.IsCancellationRequested || generation != tunnelGeneration)return;
            LaunchTunnel();
            Status.Text=stopping?"Stopping after active work finishes…":"SSH tunnel restarting (check controller for connectivity)";
        } catch(OperationCanceledException) { }
        catch(Exception e) { failure=e; }
        finally { if(generation == tunnelGeneration)tunnelRetryScheduled=false; }
        if(failure is not null && worker is not null && !token.IsCancellationRequested && generation == tunnelGeneration) {
            Log("Could not restart SSH tunnel: "+failure.Message);
            Status.Text="Could not restart SSH tunnel; retrying…";
            _=RetryTunnelAsync();
        }
    }
    public MainWindow() {
        InitializeComponent(); Directory.CreateDirectory(directory);
        Root.Text=Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),"projects");
        try { var path=Path.Combine(directory,"config.json"); if(File.Exists(path) && JsonSerializer.Deserialize<Config>(File.ReadAllText(path)) is { } c) { Node.Text=c.Node;NodeName.Text=c.Name;Root.Text=c.Root;Capacity.Text=c.Capacity.ToString();Port.Text=c.Port.ToString();SshHost.Text=c.SshHost;RemotePort.Text=(c.RemotePort > 0 ? c.RemotePort : 8080).ToString();CodexInteractive.IsChecked=c.CodexInteractive;ClaudeInteractive.IsChecked=c.ClaudeInteractive; } Token.Password=Credentials.Load(); }
        catch(Exception e) { Status.Text="Cannot load settings: "+e.Message; }
    }
    void OnStartStop(object sender, RoutedEventArgs e) { if(worker is null) Start(); else Stop(); }
    void Start() {
        if(string.IsNullOrWhiteSpace(Node.Text) || string.IsNullOrWhiteSpace(Root.Text) || string.IsNullOrWhiteSpace(Token.Password) || !int.TryParse(Capacity.Text,out var capacity) || capacity<1 || capacity>64 || !int.TryParse(Port.Text,out var port) || port<1 || port>65535 || !int.TryParse(RemotePort.Text,out var remotePort) || remotePort<1 || remotePort>65535) {Status.Text="Check the node ID, directory, credential, capacity and port.";return;}
        var engine=Path.Combine(AppContext.BaseDirectory,"xenia-engine.exe"); if(!File.Exists(engine)){Status.Text="The bundled worker engine is missing.";return;}
        try {
            Credentials.Save(Token.Password);
            File.WriteAllText(Path.Combine(directory,"config.json"),JsonSerializer.Serialize(new Config(Node.Text,NodeName.Text,Root.Text,capacity,port,SshHost.Text,remotePort,CodexInteractive.IsChecked == true,ClaudeInteractive.IsChecked == true)));
            stopFile=Path.Combine(directory,"stop-"+Guid.NewGuid().ToString("N"));
            if(!string.IsNullOrWhiteSpace(SshHost.Text)) {
                tunnelArguments=new[]{"-N","-o","BatchMode=yes","-o","ExitOnForwardFailure=yes","-o","ServerAliveInterval=15","-o","ServerAliveCountMax=3","-L",$"127.0.0.1:{port}:127.0.0.1:{remotePort}","--",SshHost.Text};
                tunnelFailures=0;tunnelGeneration++;tunnelRetryScheduled=false;tunnelLifetime=new CancellationTokenSource();LaunchTunnel();
            }
            var info=new ProcessStartInfo(engine) { UseShellExecute=false,CreateNoWindow=true,RedirectStandardError=true,RedirectStandardOutput=true };info.ArgumentList.Add("worker");
            info.Environment.Remove("XENIA_CODEX_APP_SERVER");info.Environment.Remove("XENIA_CLAUDE_CONTROL");
            if(CodexInteractive.IsChecked == true)info.Environment["XENIA_CODEX_APP_SERVER"]="1";
            if(ClaudeInteractive.IsChecked == true)info.Environment["XENIA_CLAUDE_CONTROL"]="1";
            info.Environment["XENIA_WORKER_TOKEN"]=Token.Password;info.Environment["XENIA_NODE_ID"]=Node.Text;info.Environment["XENIA_NODE_NAME"]=string.IsNullOrWhiteSpace(NodeName.Text)?Node.Text:NodeName.Text;
            info.Environment["XENIA_PROJECTS_ROOT"]=Root.Text;info.Environment["XENIA_CAPACITY"]=capacity.ToString();info.Environment["XENIA_CONTROLLER"]=$"http://127.0.0.1:{port}";info.Environment["XENIA_WORKER_DB"]=Path.Combine(directory,"worker.db");info.Environment["XENIA_STOP_FILE"]=stopFile;
            var process=new Process { StartInfo=info,EnableRaisingEvents=true };process.OutputDataReceived+=(_,e)=>Log(e.Data);process.ErrorDataReceived+=(_,e)=>Log(e.Data);process.Exited+=(_,_)=>Dispatcher.BeginInvoke(new Action(()=>Finished(process)));
            if(!process.Start())throw new InvalidOperationException("Could not start worker.");worker=process;process.BeginOutputReadLine();process.BeginErrorReadLine();Settings.IsEnabled=false;StartStop.Content="Stop worker";Status.Text="Running (check controller for connectivity)";
        } catch(Exception e) {tunnelGeneration++;tunnelRetryScheduled=false;tunnelLifetime?.Cancel();tunnelLifetime?.Dispose();tunnelLifetime=null;tunnelArguments=null;if(tunnel is {HasExited:false})tunnel.Kill();tunnel?.Dispose();tunnel=null;Status.Text="Could not start: "+e.Message;}
    }
    void Stop() { if(worker is null || stopFile is null || stopping)return;try{File.WriteAllText(stopFile,"");stopping=true;StartStop.IsEnabled=false;Status.Text="Stopping after active work finishes…";}catch(Exception e){Status.Text="Could not request stop: "+e.Message;} }
    void Finished(Process process) {tunnelGeneration++;tunnelRetryScheduled=false;tunnelLifetime?.Cancel();tunnelLifetime?.Dispose();tunnelLifetime=null;tunnelArguments=null;if(tunnel is {HasExited:false})tunnel.Kill();tunnel?.Dispose();tunnel=null;if(stopFile is not null)File.Delete(stopFile);var code=process.ExitCode;process.Dispose();worker=null;stopping=false;Settings.IsEnabled=true;StartStop.IsEnabled=true;StartStop.Content="Start worker";Status.Text=code==0?"Stopped":$"Worker exited ({code}); see worker.log.";}
    void Log(string? line) {if(line is null)return;try{lock(directory)File.AppendAllText(Path.Combine(directory,"worker.log"),line+Environment.NewLine);}catch(IOException){}}
    void OnClosing(object? sender, CancelEventArgs e) {if(worker is not null){e.Cancel=true;Stop();}}
}
