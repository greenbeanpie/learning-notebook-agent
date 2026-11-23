# Keep JNI native bridge and event callback interface from R8/ProGuard stripping
-keep class com.localnotebook.agent.core.NativeBridge {
    native <methods>;
    public *;
}

-keep interface com.localnotebook.agent.core.CoreEventListener {
    public void onCoreEvent(java.lang.String);
}

-keep class * implements com.localnotebook.agent.core.CoreEventListener {
    public void onCoreEvent(java.lang.String);
}

# Keep kotlinx.serialization serializers
-keepattributes *Annotation*, InnerClasses
-dontnote kotlinx.serialization.AnnotationsKt
-keepclassmembers class kotlinx.serialization.json.** {
    *** Companion;
}
-keepclasseswithmembers class kotlinx.serialization.json.** {
    kotlinx.serialization.KSerializer serializer(...);
}
-keep,includedescriptorclasses class com.localnotebook.agent.core.**$$serializer { *; }
-keepclassmembers class com.localnotebook.agent.core.** {
    *** Companion;
}
-keepclasseswithmembers class com.localnotebook.agent.core.** {
    kotlinx.serialization.KSerializer serializer(...);
}
