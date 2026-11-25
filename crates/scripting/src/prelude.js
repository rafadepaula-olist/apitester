(function (global) {
  "use strict";

  var init = JSON.parse(global.__apitester_init_json);
  delete global.__apitester_init_json;

  var hostLog = global.__host_log;
  var hostRecordTest = global.__host_record_test;
  var hostBtoa = global.__host_btoa;
  var hostAtob = global.__host_atob;
  var nativeJsonStringify = JSON.stringify;
  delete global.__host_log;
  delete global.__host_record_test;
  delete global.__host_btoa;
  delete global.__host_atob;

  var state = {
    globals: init.globals || {},
    collection_vars: init.collection_vars || {},
    environment_vars: init.environment_vars || {},
    local_vars: init.local_vars || {},
    data_vars: init.data_vars || {},
    next_request: null,
    stop_next_request: false,
  };

  function formatValue(val) {
    if (typeof val === "string") return val;
    if (val === null) return "null";
    if (val === undefined) return "undefined";
    if (val instanceof Error) return val.message ? (val.name + ": " + val.message) : String(val);
    if (typeof val === "object") {
      try {
        return JSON.stringify(val);
      } catch (_) {
        return String(val);
      }
    }
    return String(val);
  }

  function formatArgs(args) {
    var parts = [];
    for (var i = 0; i < args.length; i++) {
      parts.push(formatValue(args[i]));
    }
    return parts.join(" ");
  }

  global.console = {
    log: function () { hostLog("log", formatArgs(arguments)); },
    info: function () { hostLog("info", formatArgs(arguments)); },
    warn: function () { hostLog("warn", formatArgs(arguments)); },
    error: function () { hostLog("error", formatArgs(arguments)); },
  };

  global.btoa = function (input) {
    return hostBtoa(String(input));
  };

  global.atob = function (input) {
    return hostAtob(String(input));
  };

  function hasOwn(obj, key) {
    return Object.prototype.hasOwnProperty.call(obj, key);
  }

  function makeScope(store) {
    return {
      get: function (key) {
        var k = String(key);
        return hasOwn(store, k) ? store[k] : undefined;
      },
      set: function (key, value) {
        var k = String(key);
        store[k] = value === undefined ? "" : String(value);
      },
      unset: function (key) {
        delete store[String(key)];
      },
      has: function (key) {
        return hasOwn(store, String(key));
      },
      toObject: function () {
        var copy = {};
        var keys = Object.keys(store);
        for (var i = 0; i < keys.length; i++) {
          copy[keys[i]] = store[keys[i]];
        }
        return copy;
      },
      clear: function () {
        var keys = Object.keys(store);
        for (var i = 0; i < keys.length; i++) {
          delete store[keys[i]];
        }
      },
    };
  }

  function makeHeaders(list) {
    var items = Array.isArray(list) ? list : [];
    return {
      get: function (name) {
        var target = String(name).toLowerCase();
        for (var i = 0; i < items.length; i++) {
          if (items[i].key.toLowerCase() === target) {
            return items[i].value;
          }
        }
        return undefined;
      },
      has: function (name) {
        var target = String(name).toLowerCase();
        for (var i = 0; i < items.length; i++) {
          if (items[i].key.toLowerCase() === target) {
            return true;
          }
        }
        return false;
      },
      toObject: function () {
        var obj = {};
        for (var i = 0; i < items.length; i++) {
          obj[items[i].key] = items[i].value;
        }
        return obj;
      },
      all: function () {
        var out = [];
        for (var i = 0; i < items.length; i++) {
          out.push({ key: items[i].key, value: items[i].value });
        }
        return out;
      },
      count: function () {
        return items.length;
      },
    };
  }

  function typeOf(val) {
    if (val === null) return "null";
    if (val === undefined) return "undefined";
    if (Array.isArray(val)) return "array";
    if (val instanceof RegExp) return "regexp";
    if (val instanceof Date) return "date";
    if (typeof Map !== "undefined" && val instanceof Map) return "map";
    if (typeof Set !== "undefined" && val instanceof Set) return "set";
    return typeof val;
  }

  function deepEqual(a, b) {
    if (a === b) return true;
    if (Number.isNaN(a) && Number.isNaN(b)) return true;
    if (a === null || b === null || typeof a !== "object" || typeof b !== "object") {
      return false;
    }
    if (Array.isArray(a) !== Array.isArray(b)) return false;
    if (Array.isArray(a)) {
      if (a.length !== b.length) return false;
      for (var i = 0; i < a.length; i++) {
        if (!deepEqual(a[i], b[i])) return false;
      }
      return true;
    }
    if (a instanceof Date && b instanceof Date) {
      return a.getTime() === b.getTime();
    }
    if (a instanceof RegExp && b instanceof RegExp) {
      return a.source === b.source && a.flags === b.flags;
    }
    var keysA = Object.keys(a);
    var keysB = Object.keys(b);
    if (keysA.length !== keysB.length) return false;
    for (var j = 0; j < keysA.length; j++) {
      var k = keysA[j];
      if (!hasOwn(b, k) || !deepEqual(a[k], b[k])) return false;
    }
    return true;
  }

  function AssertionError(message) {
    var err = new Error(message);
    err.name = "AssertionError";
    return err;
  }

  function inspect(val) {
    if (typeof val === "string") return JSON.stringify(val);
    if (val === null) return "null";
    if (val === undefined) return "undefined";
    if (typeof val === "object") {
      try {
        return JSON.stringify(val);
      } catch (_) {
        return String(val);
      }
    }
    return String(val);
  }

  function createExpect(actualVal, customMessage) {
    var ctx = {
      actual: actualVal,
      negated: false,
      deep: false,
      message: customMessage ? String(customMessage) + ": " : "",
    };

    function check(cond, posMsg, negMsg) {
      var ok = ctx.negated ? !cond : cond;
      if (!ok) {
        throw AssertionError(ctx.message + (ctx.negated ? negMsg : posMsg));
      }
      return api;
    }

    var api = {
      equal: function (expected) {
        var pass = ctx.deep ? deepEqual(ctx.actual, expected) : ctx.actual === expected;
        return check(
          pass,
          "expected " + inspect(ctx.actual) + " to equal " + inspect(expected),
          "expected " + inspect(ctx.actual) + " to not equal " + inspect(expected)
        );
      },
      eql: function (expected) {
        return check(
          deepEqual(ctx.actual, expected),
          "expected " + inspect(ctx.actual) + " to deeply equal " + inspect(expected),
          "expected " + inspect(ctx.actual) + " to not deeply equal " + inspect(expected)
        );
      },
      a: function (expectedType) {
        var expected = String(expectedType).toLowerCase();
        var actualType = typeOf(ctx.actual);
        return check(
          actualType === expected,
          "expected " + inspect(ctx.actual) + " to be a " + expected + " (got " + actualType + ")",
          "expected " + inspect(ctx.actual) + " not to be a " + expected
        );
      },
      include: function (needle) {
        var found = false;
        if (typeof ctx.actual === "string") {
          found = ctx.actual.indexOf(String(needle)) !== -1;
        } else if (Array.isArray(ctx.actual)) {
          if (ctx.deep) {
            for (var i = 0; i < ctx.actual.length; i++) {
              if (deepEqual(ctx.actual[i], needle)) {
                found = true;
                break;
              }
            }
          } else {
            found = ctx.actual.indexOf(needle) !== -1;
          }
        } else if (ctx.actual !== null && typeof ctx.actual === "object" && needle !== null && typeof needle === "object") {
          found = true;
          var keys = Object.keys(needle);
          for (var j = 0; j < keys.length; j++) {
            var k = keys[j];
            var match = ctx.deep ? deepEqual(ctx.actual[k], needle[k]) : ctx.actual[k] === needle[k];
            if (!hasOwn(ctx.actual, k) || !match) {
              found = false;
              break;
            }
          }
        }
        return check(
          found,
          "expected " + inspect(ctx.actual) + " to include " + inspect(needle),
          "expected " + inspect(ctx.actual) + " to not include " + inspect(needle)
        );
      },
      property: function (name, expectedVal) {
        var key = String(name);
        var hasProp = ctx.actual !== null && ctx.actual !== undefined && (key in Object(ctx.actual));
        if (arguments.length >= 2) {
          var valMatches = hasProp && (ctx.deep ? deepEqual(ctx.actual[key], expectedVal) : ctx.actual[key] === expectedVal);
          check(
            valMatches,
            "expected " + inspect(ctx.actual) + " to have property " + inspect(key) + " with value " + inspect(expectedVal),
            "expected " + inspect(ctx.actual) + " to not have property " + inspect(key) + " with value " + inspect(expectedVal)
          );
        } else {
          check(
            hasProp,
            "expected " + inspect(ctx.actual) + " to have property " + inspect(key),
            "expected " + inspect(ctx.actual) + " to not have property " + inspect(key)
          );
        }
        if (hasProp) {
          ctx.actual = ctx.actual[key];
        }
        return api;
      },
      lengthOf: function (expectedLen) {
        var hasLen = ctx.actual !== null && ctx.actual !== undefined && typeof ctx.actual.length === "number";
        var actualLen = hasLen ? ctx.actual.length : undefined;
        return check(
          hasLen && actualLen === expectedLen,
          "expected " + inspect(ctx.actual) + " to have length " + expectedLen + " (got " + actualLen + ")",
          "expected " + inspect(ctx.actual) + " to not have length " + expectedLen
        );
      },
      above: function (n) {
        return check(
          typeof ctx.actual === "number" && ctx.actual > n,
          "expected " + inspect(ctx.actual) + " to be above " + n,
          "expected " + inspect(ctx.actual) + " to not be above " + n
        );
      },
      below: function (n) {
        return check(
          typeof ctx.actual === "number" && ctx.actual < n,
          "expected " + inspect(ctx.actual) + " to be below " + n,
          "expected " + inspect(ctx.actual) + " to not be below " + n
        );
      },
      least: function (n) {
        return check(
          typeof ctx.actual === "number" && ctx.actual >= n,
          "expected " + inspect(ctx.actual) + " to be at least " + n,
          "expected " + inspect(ctx.actual) + " to be below " + n
        );
      },
      most: function (n) {
        return check(
          typeof ctx.actual === "number" && ctx.actual <= n,
          "expected " + inspect(ctx.actual) + " to be at most " + n,
          "expected " + inspect(ctx.actual) + " to be above " + n
        );
      },
      match: function (re) {
        var regex = re instanceof RegExp ? re : new RegExp(re);
        return check(
          regex.test(String(ctx.actual)),
          "expected " + inspect(ctx.actual) + " to match " + String(regex),
          "expected " + inspect(ctx.actual) + " not to match " + String(regex)
        );
      },
      oneOf: function (list) {
        var ok = Array.isArray(list) && list.indexOf(ctx.actual) !== -1;
        return check(
          ok,
          "expected " + inspect(ctx.actual) + " to be one of " + inspect(list),
          "expected " + inspect(ctx.actual) + " not to be one of " + inspect(list)
        );
      },
    };

    // Aliases
    api.eq = api.equal;
    api.equals = api.equal;
    api.eqls = api.eql;
    api.an = api.a;
    api.includes = api.include;
    api.contain = api.include;
    api.contains = api.include;
    api.length = api.lengthOf;
    api.gt = api.above;
    api.greaterThan = api.above;
    api.lt = api.below;
    api.lessThan = api.below;
    api.gte = api.least;
    api.lte = api.most;

    // Chainable language words
    var chains = [
      "to", "be", "been", "is", "that", "which", "and",
      "has", "have", "with", "at", "of", "same", "but", "does", "still"
    ];
    for (var i = 0; i < chains.length; i++) {
      Object.defineProperty(api, chains[i], {
        get: function () { return api; },
        enumerable: true,
      });
    }

    // Flags
    Object.defineProperty(api, "not", {
      get: function () {
        ctx.negated = !ctx.negated;
        return api;
      },
      enumerable: true,
    });
    Object.defineProperty(api, "deep", {
      get: function () {
        ctx.deep = true;
        return api;
      },
      enumerable: true,
    });

    // Terminal boolean/value getters
    Object.defineProperty(api, "true", {
      get: function () {
        return check(
          ctx.actual === true,
          "expected " + inspect(ctx.actual) + " to be true",
          "expected " + inspect(ctx.actual) + " to not be true"
        );
      },
      enumerable: true,
    });
    Object.defineProperty(api, "false", {
      get: function () {
        return check(
          ctx.actual === false,
          "expected " + inspect(ctx.actual) + " to be false",
          "expected " + inspect(ctx.actual) + " to not be false"
        );
      },
      enumerable: true,
    });
    Object.defineProperty(api, "null", {
      get: function () {
        return check(
          ctx.actual === null,
          "expected " + inspect(ctx.actual) + " to be null",
          "expected " + inspect(ctx.actual) + " to not be null"
        );
      },
      enumerable: true,
    });
    Object.defineProperty(api, "undefined", {
      get: function () {
        return check(
          ctx.actual === undefined,
          "expected " + inspect(ctx.actual) + " to be undefined",
          "expected " + inspect(ctx.actual) + " to not be undefined"
        );
      },
      enumerable: true,
    });
    Object.defineProperty(api, "ok", {
      get: function () {
        return check(
          Boolean(ctx.actual),
          "expected " + inspect(ctx.actual) + " to be truthy",
          "expected " + inspect(ctx.actual) + " to be falsy"
        );
      },
      enumerable: true,
    });
    Object.defineProperty(api, "exist", {
      get: function () {
        return check(
          ctx.actual !== null && ctx.actual !== undefined,
          "expected " + inspect(ctx.actual) + " to exist",
          "expected " + inspect(ctx.actual) + " to not exist"
        );
      },
      enumerable: true,
    });
    Object.defineProperty(api, "empty", {
      get: function () {
        var isEmpty = false;
        if (typeof ctx.actual === "string" || Array.isArray(ctx.actual)) {
          isEmpty = ctx.actual.length === 0;
        } else if (ctx.actual !== null && typeof ctx.actual === "object") {
          isEmpty = Object.keys(ctx.actual).length === 0;
        }
        return check(
          isEmpty,
          "expected " + inspect(ctx.actual) + " to be empty",
          "expected " + inspect(ctx.actual) + " not to be empty"
        );
      },
      enumerable: true,
    });

    return api;
  }

  function makeResponseAssertion(resp) {
    var negated = false;

    function assertCond(cond, posMsg, negMsg) {
      var ok = negated ? !cond : cond;
      negated = false;
      if (!ok) {
        throw AssertionError(ok ? "" : (cond ? negMsg : posMsg));
      }
      return assertion;
    }

    var assertion = {
      status: function (expected) {
        if (typeof expected === "number") {
          return assertCond(
            resp.code === expected,
            "expected response to have status code " + expected + " but got " + resp.code,
            "expected response to not have status code " + expected
          );
        }
        return assertCond(
          resp.status === String(expected),
          "expected response to have status " + inspect(expected) + " but got " + inspect(resp.status),
          "expected response to not have status " + inspect(expected)
        );
      },
      header: function (key, expectedVal) {
        var hasH = resp.headers.has(key);
        if (arguments.length >= 2) {
          var actualVal = resp.headers.get(key);
          return assertCond(
            hasH && actualVal === String(expectedVal),
            "expected response to have header " + inspect(key) + " with value " + inspect(expectedVal) + " (got " + inspect(actualVal) + ")",
            "expected response to not have header " + inspect(key) + " with value " + inspect(expectedVal)
          );
        }
        return assertCond(
          hasH,
          "expected response to have header " + inspect(key),
          "expected response to not have header " + inspect(key)
        );
      },
      jsonBody: function (keyOrExpected, expectedVal) {
        var parsed;
        var validJson = true;
        try {
          parsed = resp.json();
        } catch (_) {
          validJson = false;
        }
        if (arguments.length === 0) {
          return assertCond(
            validJson,
            "expected response to have a valid JSON body",
            "expected response to not have a valid JSON body"
          );
        }
        if (!validJson) {
          return assertCond(false, "expected response to have a valid JSON body", "");
        }
        if (arguments.length === 1) {
          if (typeof keyOrExpected === "string") {
            return assertCond(
              parsed !== null && typeof parsed === "object" && hasOwn(parsed, keyOrExpected),
              "expected JSON body to have property " + inspect(keyOrExpected),
              "expected JSON body to not have property " + inspect(keyOrExpected)
            );
          }
          return assertCond(
            deepEqual(parsed, keyOrExpected),
            "expected JSON body " + inspect(parsed) + " to equal " + inspect(keyOrExpected),
            "expected JSON body to not equal " + inspect(keyOrExpected)
          );
        }
        return assertCond(
          parsed !== null && typeof parsed === "object" && deepEqual(parsed[keyOrExpected], expectedVal),
          "expected JSON body property " + inspect(keyOrExpected) + " to equal " + inspect(expectedVal),
          "expected JSON body property " + inspect(keyOrExpected) + " to not equal " + inspect(expectedVal)
        );
      },
    };

    var respChains = ["to", "be", "have", "has", "with", "and"];
    for (var i = 0; i < respChains.length; i++) {
      Object.defineProperty(assertion, respChains[i], {
        get: function () { return assertion; },
        enumerable: true,
      });
    }

    Object.defineProperty(assertion, "not", {
      get: function () {
        negated = !negated;
        return assertion;
      },
      enumerable: true,
    });

    Object.defineProperty(assertion, "ok", {
      get: function () {
        return assertCond(
          resp.code >= 200 && resp.code < 300,
          "expected response to be ok (2xx), got " + resp.code,
          "expected response not to be ok (2xx), got " + resp.code
        );
      },
      enumerable: true,
    });

    Object.defineProperty(assertion, "json", {
      get: function () {
        var isJson = false;
        try {
          JSON.parse(resp.text());
          isJson = true;
        } catch (_) {
          isJson = false;
        }
        return assertCond(
          isJson,
          "expected response body to be valid JSON",
          "expected response body not to be valid JSON"
        );
      },
      enumerable: true,
    });

    Object.defineProperty(assertion, "info", {
      get: function () {
        return assertCond(resp.code >= 100 && resp.code < 200, "expected 1xx response, got " + resp.code, "expected non-1xx response");
      },
      enumerable: true,
    });
    Object.defineProperty(assertion, "success", {
      get: function () {
        return assertCond(resp.code >= 200 && resp.code < 300, "expected 2xx response, got " + resp.code, "expected non-2xx response");
      },
      enumerable: true,
    });
    Object.defineProperty(assertion, "redirection", {
      get: function () {
        return assertCond(resp.code >= 300 && resp.code < 400, "expected 3xx response, got " + resp.code, "expected non-3xx response");
      },
      enumerable: true,
    });
    Object.defineProperty(assertion, "clientError", {
      get: function () {
        return assertCond(resp.code >= 400 && resp.code < 500, "expected 4xx response, got " + resp.code, "expected non-4xx response");
      },
      enumerable: true,
    });
    Object.defineProperty(assertion, "serverError", {
      get: function () {
        return assertCond(resp.code >= 500 && resp.code < 600, "expected 5xx response, got " + resp.code, "expected non-5xx response");
      },
      enumerable: true,
    });
    Object.defineProperty(assertion, "error", {
      get: function () {
        return assertCond(resp.code >= 400 && resp.code < 600, "expected 4xx/5xx response, got " + resp.code, "expected non-error response");
      },
      enumerable: true,
    });

    return assertion;
  }

  function buildResponse(rawResp) {
    if (!rawResp) return undefined;
    var bodyText = rawResp.body || "";
    var resp = {
      code: rawResp.code,
      status: rawResp.status,
      responseTime: rawResp.response_time,
      headers: makeHeaders(rawResp.headers),
      text: function () {
        return bodyText;
      },
      json: function () {
        return JSON.parse(bodyText);
      },
    };
    Object.defineProperty(resp, "to", {
      get: function () {
        return makeResponseAssertion(resp);
      },
      enumerable: true,
    });
    return resp;
  }

  function setNextRequest(name) {
    if (name === null || name === undefined) {
      state.next_request = null;
      state.stop_next_request = true;
    } else {
      state.next_request = String(name);
      state.stop_next_request = false;
    }
  }

  var pmVariables = {
    get: function (key) {
      var k = String(key);
      if (hasOwn(state.local_vars, k)) return state.local_vars[k];
      if (hasOwn(state.data_vars, k)) return state.data_vars[k];
      if (hasOwn(state.environment_vars, k)) return state.environment_vars[k];
      if (hasOwn(state.collection_vars, k)) return state.collection_vars[k];
      if (hasOwn(state.globals, k)) return state.globals[k];
      return undefined;
    },
    set: function (key, value) {
      state.local_vars[String(key)] = value === undefined ? "" : String(value);
    },
    unset: function (key) {
      delete state.local_vars[String(key)];
    },
    has: function (key) {
      var k = String(key);
      return (
        hasOwn(state.local_vars, k) ||
        hasOwn(state.data_vars, k) ||
        hasOwn(state.environment_vars, k) ||
        hasOwn(state.collection_vars, k) ||
        hasOwn(state.globals, k)
      );
    },
    toObject: function () {
      return Object.assign(
        {},
        state.globals,
        state.collection_vars,
        state.environment_vars,
        state.data_vars,
        state.local_vars
      );
    },
    replaceIn: function (template) {
      return String(template).replace(/\{\{([^}]+)\}\}/g, function (match, key) {
        var val = pmVariables.get(key.trim());
        return val === undefined ? match : val;
      });
    },
  };

  var pmIterationData = {
    get: function (key) {
      var k = String(key);
      return hasOwn(state.data_vars, k) ? state.data_vars[k] : undefined;
    },
    has: function (key) {
      return hasOwn(state.data_vars, String(key));
    },
    toObject: function () {
      return Object.assign({}, state.data_vars);
    },
  };

  var pmRequest = {
    url: init.request.url,
    method: init.request.method,
    headers: makeHeaders(init.request.headers),
    body: init.request.body,
  };

  global.pm = {
    test: function (name, fn) {
      var testName = String(name);
      try {
        fn();
        hostRecordTest(testName, true, "");
      } catch (err) {
        var msg = err && err.message ? String(err.message) : String(err);
        hostRecordTest(testName, false, msg);
      }
    },
    expect: function (actual, message) {
      return createExpect(actual, message);
    },
    response: buildResponse(init.response),
    request: pmRequest,
    variables: pmVariables,
    environment: makeScope(state.environment_vars),
    collectionVariables: makeScope(state.collection_vars),
    globals: makeScope(state.globals),
    iterationData: pmIterationData,
    execution: {
      setNextRequest: setNextRequest,
    },
  };

  global.postman = {
    setNextRequest: setNextRequest,
  };

  Object.defineProperty(global, "__apitester_dump_state", {
    value: function () {
      return nativeJsonStringify({
        globals: state.globals,
        collection_vars: state.collection_vars,
        environment_vars: state.environment_vars,
        local_vars: state.local_vars,
        next_request: state.next_request,
        stop_next_request: state.stop_next_request,
      });
    },
    writable: false,
    configurable: false,
    enumerable: false,
  });
})(globalThis);
