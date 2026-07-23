import ifcopenshell, ifcopenshell.geom, collections, sys
f = ifcopenshell.open(sys.argv[1])
print("schema:", f.schema)
prods = f.by_type("IfcElement")
print("IfcElement count:", len(prods))
for k,v in collections.Counter(p.is_a() for p in prods).most_common(20): print(f"  {v:5d} {k}")
print("storeys:", [s.Name for s in f.by_type("IfcBuildingStorey")])
print("spaces :", [ (s.Name, s.LongName) for s in f.by_type("IfcSpace")][:8], "…total", len(f.by_type("IfcSpace")))
print("space boundaries:", len(f.by_type("IfcRelSpaceBoundary")))
st = ifcopenshell.geom.settings(); st.set(st.USE_WORLD_COORDS, True)
w = (f.by_type("IfcWall")+f.by_type("IfcWallStandardCase"))
if w:
    g = ifcopenshell.geom.create_shape(st, w[0]).geometry
    v = g.verts; xs,ys,zs = v[0::3],v[1::3],v[2::3]
    print("sample wall:", w[0].Name, "world bbox min", (round(min(xs),2),round(min(ys),2),round(min(zs),2)), "max",(round(max(xs),2),round(max(ys),2),round(max(zs),2)))
