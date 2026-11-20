using LocalNotebook.Interop;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using System.Collections.ObjectModel;
using System.Text.Json;
using Windows.Storage.Pickers;
using WinRT.Interop;

namespace LocalNotebook.Views;

public sealed partial class MainWindow : Window
{
    private string? _activeNotebookId;
    private readonly ObservableCollection<string> _sources = new();
    private readonly ObservableCollection<string> _messages = new();
    private readonly ObservableCollection<string> _artifacts = new();

    public MainWindow()
    {
        InitializeComponent();
        SourcesList.ItemsSource = _sources;
        ChatList.ItemsSource = _messages;
        ArtifactsList.ItemsSource = _artifacts;
        App.Core.EventReceived += OnCoreEvent;
        Closed += (_, _) => App.Core.Dispose();
    }

    public async Task OnCoreReadyAsync()
    {
        var empty = JsonDocument.Parse("{}").RootElement;
        var resp = await App.Core.CallAsync("notebook.list", empty);
        if (resp.Result is { } res && res.TryGetProperty("notebooks", out var arr) && arr.GetArrayLength() > 0)
        {
            var first = arr[0];
            _activeNotebookId = first.GetProperty("id").GetString();
            NotebookTitleText.Text = first.GetProperty("title").GetString() ?? "未命名笔记本";
        }
        else
        {
            var createParams = JsonDocument.Parse("{\"title\":\"未命名笔记本\"}").RootElement;
            var created = await App.Core.CallAsync("notebook.create", createParams);
            if (created.Result is { } cr)
            {
                _activeNotebookId = cr.GetProperty("id").GetString();
                NotebookTitleText.Text = cr.GetProperty("title").GetString() ?? "未命名笔记本";
            }
        }
        await RefreshNotebookDataAsync();
    }

    private async Task RefreshNotebookDataAsync()
    {
        if (_activeNotebookId == null) return;
        var param = JsonDocument.Parse($"{{\"notebook_id\":\"{_activeNotebookId}\"}}").RootElement;

        var srcResp = await App.Core.CallAsync("source.list", param);
        _sources.Clear();
        if (srcResp.Result is { } sr && sr.TryGetProperty("sources", out var sArr))
        {
            foreach (var s in sArr.EnumerateArray())
            {
                _sources.Add($"{s.GetProperty("title").GetString()} [{s.GetProperty("status").GetString()}]");
            }
        }

        var chatResp = await App.Core.CallAsync("chat.history", param);
        _messages.Clear();
        if (chatResp.Result is { } cr && cr.TryGetProperty("messages", out var mArr))
        {
            foreach (var m in mArr.EnumerateArray())
            {
                _messages.Add($"{m.GetProperty("role").GetString()}: {m.GetProperty("content").GetString()}");
            }
        }

        var artResp = await App.Core.CallAsync("studio.list", param);
        _artifacts.Clear();
        if (artResp.Result is { } ar && ar.TryGetProperty("artifacts", out var aArr))
        {
            foreach (var a in aArr.EnumerateArray())
            {
                _artifacts.Add($"{a.GetProperty("title").GetString()} ({a.GetProperty("status").GetString()})");
            }
        }
    }

    private void OnCoreEvent(ApiEventDto ev)
    {
        DispatcherQueue.TryEnqueue(async () =>
        {
            if (ev.Event == "source.status" || ev.Event == "studio.status")
            {
                await RefreshNotebookDataAsync();
            }
        });
    }

    private async void OnNewNotebookClick(object sender, RoutedEventArgs e)
    {
        var createParams = JsonDocument.Parse("{\"title\":\"新笔记本\"}").RootElement;
        var resp = await App.Core.CallAsync("notebook.create", createParams);
        if (resp.Result is { } res)
        {
            _activeNotebookId = res.GetProperty("id").GetString();
            NotebookTitleText.Text = res.GetProperty("title").GetString() ?? "新笔记本";
            await RefreshNotebookDataAsync();
        }
    }

    private async void OnAddSourceClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId == null) return;
        var picker = new FileOpenPicker();
        InitializeWithWindow.Initialize(picker, WindowNative.GetWindowHandle(this));
        picker.FileTypeFilter.Add(".pdf");
        picker.FileTypeFilter.Add(".md");
        picker.FileTypeFilter.Add(".txt");
        var file = await picker.PickSingleFileAsync();
        if (file == null) return;

        var escapedPath = JsonSerializer.Serialize(file.Path, LnaJsonContext.Default.String);
        var param = JsonDocument.Parse($"{{\"notebook_id\":\"{_activeNotebookId}\",\"paths\":[{escapedPath}]}}").RootElement;
        await App.Core.CallAsync("source.add_files", param);
        await RefreshNotebookDataAsync();
    }

    private async void OnCreateNoteClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId == null) return;
        var param = JsonDocument.Parse($"{{\"notebook_id\":\"{_activeNotebookId}\",\"title\":\"新笔记\",\"body_md\":\"\"}}").RootElement;
        await App.Core.CallAsync("note.save", param);
    }

    private async void OnSendChatClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId == null || string.IsNullOrWhiteSpace(ChatInputBox.Text)) return;
        var text = ChatInputBox.Text.Trim();
        ChatInputBox.Text = string.Empty;
        _messages.Add($"user: {text}");
        var escaped = JsonSerializer.Serialize(text, LnaJsonContext.Default.String);
        var param = JsonDocument.Parse($"{{\"notebook_id\":\"{_activeNotebookId}\",\"text\":{escaped}}}").RootElement;
        await App.Core.CallAsync("chat.send", param);
        await RefreshNotebookDataAsync();
    }

    private async void OnStudioTileClick(object sender, RoutedEventArgs e)
    {
        if (_activeNotebookId == null || sender is not Button btn || btn.Tag is not string kind) return;
        var param = JsonDocument.Parse($"{{\"notebook_id\":\"{_activeNotebookId}\",\"artifact_type\":\"{kind}\",\"config\":{{}}}}").RootElement;
        await App.Core.CallAsync("studio.create", param);
        await RefreshNotebookDataAsync();
    }

    private void OnOpenSettingsClick(object sender, RoutedEventArgs e)
    {
        // Opens model profile & credential configuration dialog (profile.* / secret.*)
    }
}
