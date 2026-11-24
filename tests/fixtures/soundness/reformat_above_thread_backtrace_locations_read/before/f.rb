def f(x)
  x+1
end

def p0
  0
end

def p1
  1
end

def p2
  2
end

def p3
  3
end

def p4
  4
end

def g
  Thread.current.backtrace_locations.first.lineno
end
