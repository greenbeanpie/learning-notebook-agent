package com.localnotebook.agent

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.lifecycle.viewmodel.compose.viewModel
import com.localnotebook.agent.core.AppViewModel
import com.localnotebook.agent.ui.MainScreen
import com.localnotebook.agent.ui.theme.LocalNotebookTheme

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            val vm: AppViewModel = viewModel()
            val state by vm.ui.collectAsState()
            LocalNotebookTheme(
                themeMode = state.prefs.theme,
                baseFontSize = state.prefs.fontSize,
            ) {
                MainScreen(vm = vm)
            }
        }
    }
}
