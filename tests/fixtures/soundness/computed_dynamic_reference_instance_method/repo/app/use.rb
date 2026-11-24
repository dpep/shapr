def f(w, n)
  Widget.instance_method(n).bind(w).call
end
