class A
  def method_missing(n, *a)
    n == :show ? 1 : super
  end
  def go(a)
    show(a)
    1
  end

  def show(a)
    t = a * 2 + 1
    puts t.to_s + 'zz' + a.to_s
  end
end
