using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Text.Json;
using System.Windows;

namespace XeniaWorker;
public partial class MainWindow : Window
{
    sealed record Config(string Node, string Name, string Root, int Capacity, int Port, string SshHost, int RemotePort, bool CodexInteractive = false, bool ClaudeInteractive = false, string ControllerUrl = "", bool AllowCodex = true, bool AllowClaude = true, bool AllowCheckout = false, bool AllowSetup = false, bool AllowContainers = false, string AllowedProjects = "");
    readonly string directory = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "XeniaWorker");
    Process? worker, tunnel, pairing;
    string? pairStopFile;
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
        try { var path=Path.Combine(directory,"config.json"); if(File.Exists(path) && JsonSerializer.Deserialize<Config>(File.ReadAllText(path)) is { } c) { Node.Text=c.Node;NodeName.Text=c.Name;Root.Text=c.Root;Capacity.Text=c.Capacity.ToString();Port.Text=c.Port.ToString();SshHost.Text=c.SshHost;RemotePort.Text=(c.RemotePort > 0 ? c.RemotePort : 8080).ToString();ControllerUrl.Text=c.ControllerUrl;AllowCodex.IsChecked=c.AllowCodex;AllowClaude.IsChecked=c.AllowClaude;AllowCheckout.IsChecked=c.AllowCheckout;AllowSetup.IsChecked=c.AllowSetup;AllowContainers.IsChecked=c.AllowContainers;AllowedProjects.Text=c.AllowedProjects;CodexInteractive.IsChecked=c.CodexInteractive;ClaudeInteractive.IsChecked=c.ClaudeInteractive; } Token.Password=Credentials.Load(); }
        catch(Exception e) { Status.Text="Cannot load settings: "+e.Message; }
    }
    void OnControllerUrlChanged(object sender, System.Windows.Controls.TextChangedEventArgs e) {if(LegacyConnection is not null)LegacyConnection.Visibility=string.IsNullOrWhiteSpace(ControllerUrl.Text)?Visibility.Visible:Visibility.Collapsed;}
    void OnStartStop(object sender, RoutedEventArgs e) { if(worker is null) Start(); else Stop(); }
    void Start() {
        if(pairing is not null)return;
        if(!string.IsNullOrWhiteSpace(ControllerUrl.Text) && !ValidControllerUrl(ControllerUrl.Text.Trim())) {Status.Text="Enter an HTTPS controller origin without a path, query or credentials.";return;}
        if(string.IsNullOrWhiteSpace(Node.Text) || string.IsNullOrWhiteSpace(Root.Text) || string.IsNullOrWhiteSpace(Token.Password) || !int.TryParse(Capacity.Text,out var capacity) || capacity<1 || capacity>64 || !int.TryParse(Port.Text,out var port) || port<1 || port>65535 || !int.TryParse(RemotePort.Text,out var remotePort) || remotePort<1 || remotePort>65535) {Status.Text="Check the node ID, directory, credential, capacity and port.";return;}
        var engine=Path.Combine(AppContext.BaseDirectory,"xenia-engine.exe"); if(!File.Exists(engine)){Status.Text="The bundled worker engine is missing.";return;}
        try {
            Credentials.Save(Token.Password);
            File.WriteAllText(Path.Combine(directory,"config.json"),JsonSerializer.Serialize(new Config(Node.Text,NodeName.Text,Root.Text,capacity,port,SshHost.Text,remotePort,CodexInteractive.IsChecked == true,ClaudeInteractive.IsChecked == true,ControllerUrl.Text.Trim(),AllowCodex.IsChecked==true,AllowClaude.IsChecked==true,AllowCheckout.IsChecked==true,AllowSetup.IsChecked==true,AllowContainers.IsChecked==true,AllowedProjects.Text)));
            stopFile=Path.Combine(directory,"stop-"+Guid.NewGuid().ToString("N"));
            if(string.IsNullOrWhiteSpace(ControllerUrl.Text) && !string.IsNullOrWhiteSpace(SshHost.Text)) {
                tunnelArguments=new[]{"-N","-o","BatchMode=yes","-o","ExitOnForwardFailure=yes","-o","ServerAliveInterval=15","-o","ServerAliveCountMax=3","-L",$"127.0.0.1:{port}:127.0.0.1:{remotePort}","--",SshHost.Text};
                tunnelFailures=0;tunnelGeneration++;tunnelRetryScheduled=false;tunnelLifetime=new CancellationTokenSource();LaunchTunnel();
            }
            var info=new ProcessStartInfo(engine) { UseShellExecute=false,CreateNoWindow=true,RedirectStandardError=true,RedirectStandardOutput=true };info.ArgumentList.Add("worker");
            info.Environment.Remove("XENIA_CODEX_APP_SERVER");info.Environment.Remove("XENIA_CLAUDE_CONTROL");
            if(CodexInteractive.IsChecked == true)info.Environment["XENIA_CODEX_APP_SERVER"]="1";
            if(ClaudeInteractive.IsChecked == true)info.Environment["XENIA_CLAUDE_CONTROL"]="1";
            info.Environment["XENIA_ALLOW_CODEX"]=AllowCodex.IsChecked==true?"1":"0";info.Environment["XENIA_ALLOW_CLAUDE"]=AllowClaude.IsChecked==true?"1":"0";
            info.Environment["XENIA_ALLOW_PROJECT_CHECKOUT"]=AllowCheckout.IsChecked==true?"1":"0";info.Environment["XENIA_ALLOW_SETUP_SCRIPTS"]=AllowSetup.IsChecked==true?"1":"0";info.Environment["XENIA_ALLOW_CONTAINERS"]=AllowContainers.IsChecked==true?"1":"0";info.Environment["XENIA_ALLOWED_PROJECTS"]=AllowedProjects.Text;
            info.Environment["XENIA_WORKER_TOKEN"]=Token.Password;info.Environment["XENIA_NODE_ID"]=Node.Text;info.Environment["XENIA_NODE_NAME"]=string.IsNullOrWhiteSpace(NodeName.Text)?Node.Text:NodeName.Text;
            info.Environment["XENIA_PROJECTS_ROOT"]=Root.Text;info.Environment["XENIA_CAPACITY"]=capacity.ToString();info.Environment["XENIA_CONTROLLER"]=string.IsNullOrWhiteSpace(ControllerUrl.Text)?$"http://127.0.0.1:{port}":ControllerUrl.Text.Trim();info.Environment["XENIA_WORKER_DB"]=Path.Combine(directory,"worker.db");info.Environment["XENIA_STOP_FILE"]=stopFile;
            var process=new Process { StartInfo=info,EnableRaisingEvents=true };process.OutputDataReceived+=(_,e)=>Log(e.Data);process.ErrorDataReceived+=(_,e)=>Log(e.Data);process.Exited+=(_,_)=>Dispatcher.BeginInvoke(new Action(()=>Finished(process)));
            if(!process.Start())throw new InvalidOperationException("Could not start worker.");worker=process;process.BeginOutputReadLine();process.BeginErrorReadLine();Settings.IsEnabled=false;RequestAccess.IsEnabled=false;StartStop.Content="Stop worker";Status.Text="Running (check controller for connectivity)";
        } catch(Exception e) {tunnelGeneration++;tunnelRetryScheduled=false;tunnelLifetime?.Cancel();tunnelLifetime?.Dispose();tunnelLifetime=null;tunnelArguments=null;if(tunnel is {HasExited:false})tunnel.Kill();tunnel?.Dispose();tunnel=null;Status.Text="Could not start: "+e.Message;}
    }
    void Stop() { if(worker is null || stopFile is null || stopping)return;try{File.WriteAllText(stopFile,"");stopping=true;StartStop.IsEnabled=false;Status.Text="Stopping after active work finishes…";}catch(Exception e){Status.Text="Could not request stop: "+e.Message;} }
    void Finished(Process process) {tunnelGeneration++;tunnelRetryScheduled=false;tunnelLifetime?.Cancel();tunnelLifetime?.Dispose();tunnelLifetime=null;tunnelArguments=null;if(tunnel is {HasExited:false})tunnel.Kill();tunnel?.Dispose();tunnel=null;if(stopFile is not null)File.Delete(stopFile);var code=process.ExitCode;process.Dispose();worker=null;stopping=false;Settings.IsEnabled=true;RequestAccess.IsEnabled=true;StartStop.IsEnabled=true;StartStop.Content="Start worker";Status.Text=code==0?"Stopped":$"Worker exited ({code}); see worker.log.";}
    void CancelPairing() {try {if(pairStopFile is not null)File.WriteAllText(pairStopFile,"");RequestAccess.IsEnabled=false;PairingStatus.Text="Cancelling request…";}catch(Exception error){PairingStatus.Text="Could not cancel request: "+error.Message;}}
    static bool ValidControllerUrl(string value) => Uri.TryCreate(value,UriKind.Absolute,out var uri) && uri.Scheme == "https" && string.IsNullOrEmpty(uri.UserInfo) && uri.AbsolutePath == "/" && string.IsNullOrEmpty(uri.Query) && string.IsNullOrEmpty(uri.Fragment);
    async void OnRequestAccess(object sender, RoutedEventArgs e) {
        if(pairing is not null) { CancelPairing(); return; }
        if(worker is not null)return;
        if(!ValidControllerUrl(ControllerUrl.Text.Trim()) || string.IsNullOrWhiteSpace(Node.Text)) {PairingStatus.Text="Enter the controller HTTPS address and a node ID first.";return;}
        Node.Text=Node.Text.Trim();ControllerUrl.Text=ControllerUrl.Text.Trim();
        var info=new ProcessStartInfo(Path.Combine(AppContext.BaseDirectory,"xenia-engine.exe")) {UseShellExecute=false,CreateNoWindow=true,RedirectStandardOutput=true,RedirectStandardError=true};
        info.ArgumentList.Add("pair");
        foreach(var key in info.Environment.Keys.Where(k=>k.StartsWith("XENIA_",StringComparison.OrdinalIgnoreCase)).ToArray())info.Environment.Remove(key);
        pairStopFile=Path.Combine(directory,"pair-stop-"+Guid.NewGuid().ToString("N"));
        info.Environment["XENIA_CONTROLLER"]=ControllerUrl.Text.Trim();info.Environment["XENIA_NODE_ID"]=Node.Text.Trim();info.Environment["XENIA_NODE_NAME"]=string.IsNullOrWhiteSpace(NodeName.Text)?Node.Text.Trim():NodeName.Text.Trim();info.Environment["XENIA_PAIR_STOP_FILE"]=pairStopFile;
        using var process=new Process {StartInfo=info};
        try {
            process.Start();pairing=process;Settings.IsEnabled=false;StartStop.IsEnabled=false;RequestAccess.Content="Cancel request";PairingStatus.Text="Requesting access…";
            var errors=process.StandardError.ReadToEndAsync();bool completed=false;
            while(await process.StandardOutput.ReadLineAsync() is { } line) {
                using var json=JsonDocument.Parse(line);var data=json.RootElement;
                if(data.GetProperty("state").GetString()=="completed") {
                    var credential=data.GetProperty("token").GetString() ?? throw new InvalidOperationException("Missing credential");
                    Token.Password=credential;Credentials.Save(credential);completed=true;
                    File.WriteAllText(Path.Combine(directory,"config.json"),JsonSerializer.Serialize(new Config(Node.Text.Trim(),NodeName.Text,Root.Text,int.TryParse(Capacity.Text,out var c)?c:1,int.TryParse(Port.Text,out var p)?p:8080,SshHost.Text,int.TryParse(RemotePort.Text,out var r)?r:8080,CodexInteractive.IsChecked==true,ClaudeInteractive.IsChecked==true,ControllerUrl.Text.Trim(),AllowCodex.IsChecked==true,AllowClaude.IsChecked==true,AllowCheckout.IsChecked==true,AllowSetup.IsChecked==true,AllowContainers.IsChecked==true,AllowedProjects.Text)));
                    PairingStatus.Text="Approved. Credential saved in Windows Credential Manager. Choose Start worker.";
                } else PairingStatus.Text="Compare code "+data.GetProperty("code").GetString()+" in the controller’s Nodes view and approve this worker. Expires in five minutes.";
            }
            await process.WaitForExitAsync();var error=await errors;
            if(!completed)PairingStatus.Text=string.IsNullOrWhiteSpace(error)?"Request ended; request access again.":error.Trim();
        } catch(Exception error) {PairingStatus.Text="Pairing failed: "+error.Message; if(pairing is not null && !process.HasExited)process.Kill();}
        finally {pairing=null;if(pairStopFile is not null)File.Delete(pairStopFile);pairStopFile=null;Settings.IsEnabled=true;StartStop.IsEnabled=true;RequestAccess.IsEnabled=true;RequestAccess.Content="Request access";}
    }
    void Log(string? line) {if(line is null)return;try{lock(directory)File.AppendAllText(Path.Combine(directory,"worker.log"),line+Environment.NewLine);}catch(IOException){}}
    void OnClosing(object? sender, CancelEventArgs e) {if(pairing is not null){e.Cancel=true;CancelPairing();return;}if(worker is not null){e.Cancel=true;Stop();}}
}
