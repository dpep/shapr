class A
  def go(a)
    call(a)
    1
  end

  def call(a)
    t = a * 2 + 1
    u = t + a * 3
    v = u - t
  end
end
