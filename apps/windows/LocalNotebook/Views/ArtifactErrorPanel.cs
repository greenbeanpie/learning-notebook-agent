using LocalNotebook.Services;
using Microsoft.UI.Text;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using System.Text.Json;
using Windows.ApplicationModel.DataTransfer;
using static LocalNotebook.Services.Api;

namespace LocalNotebook.Views;

/// <summary>
/// Shared Studio error display with clean user-facing copy, one-click retry,
/// and a collapsed-by-default diagnostic log that can be expanded and copied.
/// </summary>
internal static class ArtifactErrorPanel
{
    public static FrameworkElement Build(
        JsonElement artifact,
        Action<InfoBarSeverity, string, string> notify,
        Func<Task> onRetry,
        Action? onViewProcess = null,
        bool compact = false)
    {
        var code = Str(artifact, "error_code");
        var detail = Str(artifact, "error_detail");
        var advice = code.Length > 0 ? Advice(code) : "";
        var headline = advice.Length > 0
            ? advice
            : L10n.T("studio.gen_failed_msg");

        var errorLog = Str(artifact, "error_log");
        if (errorLog.Length == 0)
        {
            errorLog = $"ArtifactId: {Str(artifact, "id")}\nType: {Str(artifact, "artifact_type")}\nStatus: {Str(artifact, "status")}\nErrorCode: {code}\nErrorDetail: {detail}";
        }

        var root = new StackPanel { Spacing = 8 };

        root.Children.Add(new TextBlock
        {
            Text = headline,
            FontSize = compact ? 12 : 13,
            FontWeight = FontWeights.Medium,
            TextWrapping = TextWrapping.Wrap,
            Foreground = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 255, 160, 140)),
        });

        var actions = new StackPanel
        {
            Orientation = Orientation.Horizontal,
            Spacing = 8,
        };

        var retryBtn = new Button
        {
            Content = L10n.T("studio.retry"),
            Style = (Style)Application.Current.Resources["PrimaryPillButtonStyle"],
            MinHeight = compact ? 30 : 34,
            Padding = new Thickness(14, 4, 14, 4),
            FontSize = 12,
        };
        retryBtn.Click += async (_, _) =>
        {
            retryBtn.IsEnabled = false;
            retryBtn.Content = L10n.T("studio.retrying");
            try
            {
                await onRetry();
            }
            finally
            {
                retryBtn.IsEnabled = true;
                retryBtn.Content = L10n.T("studio.retry");
            }
        };
        actions.Children.Add(retryBtn);

        if (onViewProcess != null)
        {
            var procBtn = new Button
            {
                Content = L10n.T("studio.view_process"),
                Style = (Style)Application.Current.Resources["PillButtonStyle"],
                MinHeight = compact ? 30 : 34,
                Padding = new Thickness(12, 4, 12, 4),
                FontSize = 12,
            };
            procBtn.Click += (_, _) => onViewProcess();
            actions.Children.Add(procBtn);
        }

        root.Children.Add(actions);

        var copyBtn = new Button
        {
            Content = L10n.T("studio.copy_log"),
            Style = (Style)Application.Current.Resources["PillButtonStyle"],
            MinHeight = 28,
            Padding = new Thickness(12, 3, 12, 3),
            FontSize = 12,
            HorizontalAlignment = HorizontalAlignment.Left,
        };
        copyBtn.Click += (_, _) =>
        {
            var dp = new DataPackage();
            dp.SetText(errorLog);
            Clipboard.SetContent(dp);
            notify(InfoBarSeverity.Success, L10n.T("studio.log_copied"), L10n.T("studio.log_copied_msg"));
        };

        var logBox = new Border
        {
            Background = (Brush)Application.Current.Resources["PanelBgBrush"],
            BorderBrush = (Brush)Application.Current.Resources["SubtleBorderBrush"],
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(8),
            Padding = new Thickness(10),
            Child = new ScrollViewer
            {
                MaxHeight = compact ? 180 : 280,
                VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
                HorizontalScrollBarVisibility = ScrollBarVisibility.Auto,
                Content = new TextBlock
                {
                    Text = errorLog,
                    FontFamily = new FontFamily("Consolas"),
                    FontSize = 11,
                    TextWrapping = TextWrapping.NoWrap,
                    IsTextSelectionEnabled = true,
                    Foreground = (Brush)Application.Current.Resources["TextSecondaryBrush"],
                },
            },
        };

        var logContent = new StackPanel { Spacing = 8 };
        logContent.Children.Add(copyBtn);
        logContent.Children.Add(logBox);

        root.Children.Add(new Expander
        {
            Header = L10n.T("studio.diag_log"),
            IsExpanded = false,
            HorizontalAlignment = HorizontalAlignment.Stretch,
            HorizontalContentAlignment = HorizontalAlignment.Stretch,
            Content = logContent,
        });

        return root;
    }
}
