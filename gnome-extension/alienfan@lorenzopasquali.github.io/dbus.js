// D-Bus access to alienfand (SPEC 9).

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

export const BUS_NAME = 'io.github.lorenzopasquali.AlienFan';
export const OBJECT_PATH = '/io/github/lorenzopasquali/AlienFan';

// Exact copy of crates/alienfan-proto/io.github.lorenzopasquali.AlienFan1.xml;
// `cargo test -p alienfan-proto` fails when they differ.
const INTERFACE_XML = `<!--
  alienfan D-Bus API (SPEC 9), session bus.
  Bus name io.github.lorenzopasquali.AlienFan, object /io/github/lorenzopasquali/AlienFan.
  Only additions are allowed; a breaking change becomes AlienFan2.
  Additions to the SPEC 9 draft are marked "v1 addition".
-->
<node>
  <interface name="io.github.lorenzopasquali.AlienFan1">
    <!-- Every property emits PropertiesChanged. -->
    <property name="Version" type="s" access="read"/>
    <!-- ok|degraded|emergency|no-permission|no-driver -->
    <property name="Health" type="s" access="read"/>
    <!-- pt-BR text for the UI; empty when Health is ok -->
    <property name="HealthMessage" type="s" access="read"/>
    <!-- ac|battery -->
    <property name="PowerSource" type="s" access="read"/>
    <!-- profile read from sysfs -->
    <property name="Profile" type="s" access="read"/>
    <property name="AvailableProfiles" type="as" access="read"/>
    <!-- firmware|fixed|curve -->
    <property name="Control" type="s" access="read"/>
    <!-- "" unless Control is curve -->
    <property name="ActiveCurve" type="s" access="read"/>
    <property name="OverrideActive" type="b" access="read"/>
    <property name="BoostRequiresCustom" type="b" access="read"/>
    <!-- v1 addition: power-change|manual -->
    <property name="OverrideUntil" type="s" access="read"/>
    <!-- v1 addition: both fans go to boost 255 above this -->
    <property name="EmergencyTempC" type="d" access="read"/>

    <!-- fans: id s, label s, rpm u, rpm_max u, boost y, target_boost y, temp_c d, sensor s -->
    <method name="GetTelemetry">
      <arg name="fans" type="aa{sv}" direction="out"/>
      <arg name="temps" type="a{sd}" direction="out"/>
    </method>
    <!-- profile s, control s, fixed_cpu y, fixed_gpu y, curve s -->
    <method name="GetDefaults">
      <arg name="ac" type="a{sv}" direction="out"/>
      <arg name="battery" type="a{sv}" direction="out"/>
    </method>
    <method name="ListCurves">
      <arg name="names" type="as" direction="out"/>
    </method>
    <!-- fan: cpu|gpu; points: (temp °C, boost 0–255) -->
    <method name="GetCurve">
      <arg name="name" type="s" direction="in"/>
      <arg name="fan" type="s" direction="in"/>
      <arg name="points" type="a(dy)" direction="out"/>
    </method>
    <!-- v1 addition. hysteresis_c d, ramp_up_per_s q, ramp_down_per_s q -->
    <method name="GetCurveOptions">
      <arg name="name" type="s" direction="in"/>
      <arg name="options" type="a{sv}" direction="out"/>
    </method>

    <!-- The next four create or update the override. -->
    <method name="SetProfile">
      <arg name="profile" type="s" direction="in"/>
    </method>
    <!-- fan: cpu|gpu|all. Implies Control=fixed. -->
    <method name="SetFixedBoost">
      <arg name="fan" type="s" direction="in"/>
      <arg name="boost" type="y" direction="in"/>
    </method>
    <!-- control: firmware|fixed|curve. curve is ignored unless control is
         curve; empty means the curve named by the current default. -->
    <method name="SetControl">
      <arg name="control" type="s" direction="in"/>
      <arg name="curve" type="s" direction="in"/>
    </method>
    <method name="RestoreDefault"/>

    <!-- target: ac|battery|both -->
    <method name="SaveAsDefault">
      <arg name="target" type="s" direction="in"/>
    </method>
    <!-- Edits a default without creating an override. Keys as in
         GetDefaults; missing keys keep their value. -->
    <method name="SetDefault">
      <arg name="target" type="s" direction="in"/>
      <arg name="preset" type="a{sv}" direction="in"/>
    </method>
    <!-- Creates or replaces [curves.<name>], keeping its options. -->
    <method name="SaveCurve">
      <arg name="name" type="s" direction="in"/>
      <arg name="cpu" type="a(dy)" direction="in"/>
      <arg name="gpu" type="a(dy)" direction="in"/>
    </method>
    <!-- v1 addition: SaveCurve plus the options of GetCurveOptions. -->
    <method name="SaveCurveWithOptions">
      <arg name="name" type="s" direction="in"/>
      <arg name="cpu" type="a(dy)" direction="in"/>
      <arg name="gpu" type="a(dy)" direction="in"/>
      <arg name="options" type="a{sv}" direction="in"/>
    </method>
    <!-- Fails with CurveInUse if a default or the override uses it. -->
    <method name="DeleteCurve">
      <arg name="name" type="s" direction="in"/>
    </method>
    <method name="ReloadConfig"/>
    <!-- v1 addition. Accepts only name "override_until" with a string. -->
    <method name="SetDaemonOption">
      <arg name="name" type="s" direction="in"/>
      <arg name="value" type="v" direction="in"/>
    </method>

    <!-- Emitted every tick. -->
    <signal name="Telemetry">
      <arg name="fans" type="aa{sv}"/>
      <arg name="temps" type="a{sd}"/>
    </signal>
  </interface>
</node>
`;

const AlienFanProxy = Gio.DBusProxy.makeProxyWrapper(INTERFACE_XML);

/**
 * Creates the proxy asynchronously. It never starts the daemon: a stopped
 * daemon shows as "Serviço parado" until the user starts it.
 *
 * @param {Function} callback - called with (proxy, error)
 * @param {Gio.Cancellable} cancellable - cancelled on disable()
 * @returns {Gio.DBusProxy}
 */
export function createProxy(callback, cancellable) {
    return new AlienFanProxy(Gio.DBus.session, BUS_NAME, OBJECT_PATH,
        callback, cancellable, Gio.DBusProxyFlags.DO_NOT_AUTO_START);
}

/**
 * Turns an unpacked a{sv} (values still GLib.Variant) into a plain object.
 *
 * @param {object} dict - as delivered by the proxy
 * @returns {object}
 */
export function unpackDict(dict) {
    return Object.fromEntries(Object.entries(dict).map(([key, value]) =>
        [key, value instanceof GLib.Variant ? value.recursiveUnpack() : value]));
}

/**
 * The daemon's pt-BR message, without the D-Bus error name.
 *
 * @param {Error} error - from a failed call
 * @returns {string}
 */
export function errorMessage(error) {
    if (error instanceof GLib.Error)
        Gio.DBusError.strip_remote_error(error);
    return error.message;
}
