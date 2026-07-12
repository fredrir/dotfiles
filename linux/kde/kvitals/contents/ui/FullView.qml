import QtQuick
import QtQuick.Layouts
import org.kde.plasma.components as PlasmaComponents
import org.kde.kirigami as Kirigami

ColumnLayout {
    id: fullView
    spacing: Kirigami.Units.smallSpacing
    Layout.preferredWidth: Kirigami.Units.gridUnit * 18
    Layout.preferredHeight: Kirigami.Units.gridUnit * 12

    required property var metricsModel
    required property color baseTextColor
    required property bool fontBold
    required property int effectiveFontSize
    required property string fontFamily

    PlasmaComponents.Label {
        text: "KVitals"
        font.bold: true // intentional: title always bold for visual hierarchy
        font.family: fullView.fontFamily
        font.pixelSize: fullView.effectiveFontSize
        Layout.alignment: Qt.AlignHCenter
        Layout.bottomMargin: Kirigami.Units.smallSpacing
    }

    Repeater {
        model: fullView.metricsModel

        delegate: RowLayout {
            required property var modelData

            Layout.fillWidth: true
            Layout.leftMargin: Kirigami.Units.largeSpacing
            Layout.rightMargin: Kirigami.Units.largeSpacing

            PlasmaComponents.Label {
                text: modelData.label
                color: fullView.baseTextColor
                opacity: 0.7
                font.family: fullView.fontFamily
                font.pixelSize: fullView.effectiveFontSize
                Layout.fillWidth: true
            }
            PlasmaComponents.Label {
                text: modelData.value
                font.bold: fullView.fontBold
                font.family: fullView.fontFamily
                font.pixelSize: fullView.effectiveFontSize
                color: modelData.color
                horizontalAlignment: Text.AlignRight
            }
        }
    }
}
