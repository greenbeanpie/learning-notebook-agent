using Microsoft.UI.Text;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using System.Text.Json;
using Windows.ApplicationModel.DataTransfer;
using static LocalNotebook.Services.Api;

namespace LocalNotebook.Views;

/// <summary>
/// Shared Studio error display with one-click retry and a collapsed-by-default
/// developer diagnostic log that can be expanded and copied.
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
            : detail.Length > 0
                ? detail
                : "生成失败（已尝试回退重试 3 次），请重试或查看诊断日志。";

        var errorLog = Str(artifact, "error_log");
        if (errorLog.Length == 0)
        {
            errorLog = $"ArtifactId: {Str(artifact, "id")}\nType: {Str(artifact, "artifact_type")}\nStatus: {Str(artifact, "status")}\nErrorCode: {code}\nErrorDetail: {detail}";
        }

        var root = new StackPanel { Spacing = 8 };

        root.Children.Add(new TextBlock
        {
            Text = $"生成失败：{headline}",
            FontSize = compact ? 12 : 13,
            FontWeight = FontWeights.Medium,
            TextWrapping = TextWrapping.Wrap,
            Foreground = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 255, 160, 140)),
        });

        if (detail.Length > 0 && advice.Length > 0 && detail != advice)
        {
            root.Children.Add(new TextBlock
            {
                Text = $"错误摘要（{code}）：{detail}",
                FontSize = 11,
                TextWrapping = TextWrapping.Wrap,
                Foreground = (Brush)Application.Current.Resources["TextMutedBrush"],
            });
        }

        var actions = new StackPanel
        {
            Orientation = Orientation.Horizontal,
            Spacing = 8,
        };

        var retryBtn = new Button
        {
            Content = "重试",
            Style = (Style)Application.Current.Resources["PrimaryPillButtonStyle"],
            MinHeight = compact ? 30 : 34,
            Padding = new Thickness(14, 4, 14, 4),
            FontSize = 12,
        };
        retryBtn.Click += async (_, _) =>
        {
            retryBtn.IsEnabled = false;
            retryBtn.Content = "重试中...";
            try
            {
                await onRetry();
            }
            finally
            {
                retryBtn.IsEnabled = true;
                retryBtn.Content = "重试";
            }
        };
        actions.Children.Add(retryBtn);

        if (onViewProcess != null)
        {
            var procBtn = new Button
            {
                Content = "查看思考过程",
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
            Content = "复制日志",
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
            notify(InfoBarSeverity.Success, "已复制日志", "完整诊断日志已复制到剪贴板，可发送给开发者排查。");
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
            Header = "开发者日志（点击展开）",
            IsExpanded = false,
            HorizontalAlignment = HorizontalAlignment.Stretch,
            HorizontalContentAlignment = HorizontalAlignment.Stretch,
            Content = logContent,
        });

        return root;
    }
}
